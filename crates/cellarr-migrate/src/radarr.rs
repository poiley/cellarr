//! Reading a Radarr database into the source-agnostic [`MappedInstall`].
//!
//! Radarr is a flat movie library: each `Movies` row is one [`ContentKind::Movie`]
//! content node, and its `MovieFiles` row (when present) is the media file that
//! satisfies it, recognized in place at its current path. External ids
//! (TMDB/IMDB) and the title/year ride along into the typed `movie_meta` row.

use cellarr_core::{
    ContentId, ContentKind, ContentNode, Coordinates, LibraryId, MediaFile, MediaFileId, MediaType,
    QualityProfileId, QualityRanking,
};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::config_read;
use crate::error::Result;
use crate::model::{ExternalIds, MappedContent, MappedFile, MappedInstall};
use crate::profiles::{
    collect_format_scores, map_custom_formats, map_file_quality, map_quality_profile,
    SourceCustomFormat, SourceQualityProfile,
};
use crate::source::{opt_text, SourceKind};

/// Read a Radarr database into a [`MappedInstall`].
///
/// # Errors
/// Returns a [`crate::MigrationError`] on query, JSON, or mapping failure.
pub(crate) async fn read(pool: &SqlitePool) -> Result<MappedInstall> {
    let ranking = QualityRanking::default();

    let profiles = read_profiles(pool).await?;
    let scores = collect_format_scores(&profiles);
    let source_cfs = read_custom_formats(pool).await?;
    let custom_formats =
        map_custom_formats(&source_cfs, &scores, cellarr_decide::TrashApp::Radarr)?;
    let mapped_profiles = profiles
        .iter()
        .map(|p| map_quality_profile(p, &ranking))
        .collect::<Result<Vec<_>>>()?;

    let library_id = LibraryId::new();
    let default_profile = mapped_profiles
        .first()
        .map_or_else(QualityProfileId::new, |p| p.id);

    let root_folders = config_read::read_root_folders(pool).await?;
    let library = cellarr_core::Library {
        id: library_id,
        media_type: MediaType::Movie,
        name: "Movies".to_string(),
        root_folders: root_folders.iter().map(|r| r.path.clone()).collect(),
        default_quality_profile: default_profile,
    };

    let (contents, files) = read_movies(pool, library_id, &ranking).await?;

    Ok(MappedInstall {
        kind: Some(SourceKind::Radarr),
        libraries: vec![library],
        contents,
        files,
        profiles: mapped_profiles,
        custom_formats,
        indexers: config_read::read_indexers(pool, SourceKind::Radarr).await?,
        download_clients: config_read::read_download_clients(pool, SourceKind::Radarr).await?,
        root_folders,
    })
}

/// Read `Movies` + their `MovieFiles`, producing content nodes and linked files.
async fn read_movies(
    pool: &SqlitePool,
    library_id: LibraryId,
    ranking: &QualityRanking,
) -> Result<(Vec<MappedContent>, Vec<MappedFile>)> {
    // Newer Radarr keeps title/ids on a MovieMetadata side-table; older keeps
    // them on Movies directly. The representative subset we read (and our
    // fixtures) carry the columns inline on Movies, which is the schema-stable
    // shape across the versions we target.
    let rows = sqlx::query(
        "SELECT m.Id           AS id,
                m.Title        AS title,
                m.Year         AS year,
                m.TmdbId       AS tmdb_id,
                m.ImdbId       AS imdb_id,
                m.Monitored    AS monitored,
                m.MovieFileId  AS movie_file_id
         FROM Movies m
         ORDER BY m.Id ASC",
    )
    .fetch_all(pool)
    .await?;

    let mut contents = Vec::with_capacity(rows.len());
    let mut files = Vec::new();

    for row in &rows {
        let movie_id: i64 = row.try_get("id")?;
        let title: String = row.try_get("title").unwrap_or_default();
        let year: Option<i64> = row.try_get("year").ok().flatten();
        let tmdb_id: Option<i64> = row.try_get("tmdb_id").ok().flatten();
        let imdb_id = opt_text(row, "imdb_id");
        let monitored: i64 = row.try_get("monitored").unwrap_or(0);
        let movie_file_id: Option<i64> = row.try_get("movie_file_id").ok().flatten();

        let content_index = contents.len();
        let node = ContentNode {
            id: ContentId::new(),
            library_id,
            media_type: MediaType::Movie,
            parent_id: None,
            kind: ContentKind::Movie,
            coords: Coordinates::Movie,
            monitored: monitored != 0,
            title_id: None,
            tags: Vec::new(),
        };
        contents.push(MappedContent {
            node,
            title,
            year,
            external_ids: ExternalIds {
                tmdb_id,
                tvdb_id: None,
                imdb_id,
            },
        });

        if movie_file_id.is_some_and(|id| id != 0) {
            if let Some(file) = read_movie_file(pool, movie_id, ranking).await? {
                files.push(MappedFile {
                    file,
                    content_indices: vec![content_index],
                });
            }
        }
    }

    Ok((contents, files))
}

/// Read the single `MovieFiles` row for a movie, if present.
async fn read_movie_file(
    pool: &SqlitePool,
    movie_id: i64,
    ranking: &QualityRanking,
) -> Result<Option<MediaFile>> {
    let row = sqlx::query(
        "SELECT Id AS id, Path AS path, Size AS size, Quality AS quality, Languages AS languages
         FROM MovieFiles WHERE MovieId = ?1 LIMIT 1",
    )
    .bind(movie_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };

    let path: String = row.try_get("path").unwrap_or_default();
    let size: i64 = row.try_get("size").unwrap_or(0);
    let quality_json = opt_text(&row, "quality").unwrap_or_else(|| "{}".to_string());
    let quality = map_file_quality(
        &quality_json,
        ranking,
        &format!("Radarr MovieFile for movie {movie_id} Quality"),
    )?;

    Ok(Some(MediaFile {
        id: MediaFileId::new(),
        path,
        size: size.max(0) as u64,
        quality,
        languages: Vec::new(),
        media_info: None,
        custom_format_score: None,
        // An imported existing file has no grab provenance; its release type is
        // unknown until a later reconcile pass attributes one.
        release_type: None,
    }))
}

/// Read Radarr `QualityProfiles`.
async fn read_profiles(pool: &SqlitePool) -> Result<Vec<SourceQualityProfile>> {
    let rows = sqlx::query(
        "SELECT Id, Name, Items, Cutoff, MinFormatScore, CutoffFormatScore,
                FormatItems, UpgradeAllowed
         FROM QualityProfiles ORDER BY Id ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| SourceQualityProfile {
            name: row.try_get("Name").unwrap_or_default(),
            items_json: row.try_get("Items").unwrap_or_else(|_| "[]".to_string()),
            cutoff: row.try_get("Cutoff").ok().flatten(),
            min_format_score: row.try_get("MinFormatScore").unwrap_or(0),
            cutoff_format_score: row.try_get("CutoffFormatScore").unwrap_or(0),
            format_items_json: opt_text(row, "FormatItems"),
            upgrade_allowed: row.try_get::<i64, _>("UpgradeAllowed").unwrap_or(1) != 0,
        })
        .collect())
}

/// Read Radarr `CustomFormats`.
async fn read_custom_formats(pool: &SqlitePool) -> Result<Vec<SourceCustomFormat>> {
    let rows = sqlx::query("SELECT Id, Name, Specifications FROM CustomFormats ORDER BY Id ASC")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(|row| SourceCustomFormat {
            id: row.try_get("Id").unwrap_or(0),
            name: row.try_get("Name").unwrap_or_default(),
            specifications_json: row
                .try_get("Specifications")
                .unwrap_or_else(|_| "[]".to_string()),
        })
        .collect())
}
