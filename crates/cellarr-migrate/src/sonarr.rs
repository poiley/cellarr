//! Reading a Sonarr database into the source-agnostic [`MappedInstall`].
//!
//! Sonarr is a three-level tree: a `Series` row is the root content node, with a
//! `Season` node per distinct season and an `Episode` node per episode. The
//! adjacency list is emitted parent-before-child (series, then its seasons, then
//! their episodes) so a single ordered insert never violates the
//! `content.parent_id` foreign key on import. `EpisodeFiles` recognize the
//! existing files in place, linked to the episode node(s) they cover.

use std::collections::HashMap;

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

/// Read a Sonarr database into a [`MappedInstall`].
///
/// # Errors
/// Returns a [`crate::MigrationError`] on query, JSON, or mapping failure.
pub(crate) async fn read(pool: &SqlitePool) -> Result<MappedInstall> {
    let ranking = QualityRanking::default();

    let profiles = read_profiles(pool).await?;
    let scores = collect_format_scores(&profiles);
    let source_cfs = read_custom_formats(pool).await?;
    let custom_formats = map_custom_formats(&source_cfs, &scores)?;
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
        media_type: MediaType::Tv,
        name: "TV".to_string(),
        root_folders: root_folders.iter().map(|r| r.path.clone()).collect(),
        default_quality_profile: default_profile,
    };

    let (contents, files) = read_series(pool, library_id, &ranking).await?;

    Ok(MappedInstall {
        kind: Some(SourceKind::Sonarr),
        libraries: vec![library],
        contents,
        files,
        profiles: mapped_profiles,
        custom_formats,
        indexers: config_read::read_indexers(pool, SourceKind::Sonarr).await?,
        download_clients: config_read::read_download_clients(pool, SourceKind::Sonarr).await?,
        root_folders,
    })
}

/// Read every series and fan out its season/episode tree.
async fn read_series(
    pool: &SqlitePool,
    library_id: LibraryId,
    ranking: &QualityRanking,
) -> Result<(Vec<MappedContent>, Vec<MappedFile>)> {
    let series_rows = sqlx::query(
        "SELECT Id AS id, Title AS title, Year AS year, TvdbId AS tvdb_id,
                TmdbId AS tmdb_id, ImdbId AS imdb_id, Monitored AS monitored
         FROM Series ORDER BY Id ASC",
    )
    .fetch_all(pool)
    .await?;

    let mut contents = Vec::new();
    let mut files = Vec::new();

    for srow in &series_rows {
        let series_id: i64 = srow.try_get("id")?;
        let series_title: String = srow.try_get("title").unwrap_or_default();
        let series_year: Option<i64> = srow.try_get("year").ok().flatten();
        let monitored: i64 = srow.try_get("monitored").unwrap_or(0);

        let series_content_id = ContentId::new();
        contents.push(MappedContent {
            node: ContentNode {
                id: series_content_id,
                library_id,
                media_type: MediaType::Tv,
                parent_id: None,
                kind: ContentKind::Series,
                // A series root carries no per-episode numbering; use season 0 as
                // the structural placeholder coordinate (it is never grabbable).
                coords: Coordinates::Episode {
                    season: 0,
                    episode: 0,
                    absolute: None,
                },
                monitored: monitored != 0,
                title_id: None,
            },
            title: series_title.clone(),
            year: series_year,
            external_ids: ExternalIds {
                tmdb_id: srow.try_get("tmdb_id").ok().flatten(),
                tvdb_id: srow.try_get("tvdb_id").ok().flatten(),
                imdb_id: opt_text(srow, "imdb_id"),
            },
        });

        read_episodes_for_series(
            pool,
            library_id,
            series_id,
            series_content_id,
            &series_title,
            ranking,
            &mut contents,
            &mut files,
        )
        .await?;
    }

    Ok((contents, files))
}

/// Read one series' episodes, creating the season nodes on demand and the
/// per-episode nodes + their recognized files.
#[allow(clippy::too_many_arguments)]
async fn read_episodes_for_series(
    pool: &SqlitePool,
    library_id: LibraryId,
    series_id: i64,
    series_content_id: ContentId,
    series_title: &str,
    ranking: &QualityRanking,
    contents: &mut Vec<MappedContent>,
    files: &mut Vec<MappedFile>,
) -> Result<()> {
    let episode_rows = sqlx::query(
        "SELECT Id AS id, SeasonNumber AS season, EpisodeNumber AS episode,
                AbsoluteEpisodeNumber AS absolute, Title AS title, Monitored AS monitored,
                EpisodeFileId AS file_id
         FROM Episodes WHERE SeriesId = ?1
         ORDER BY SeasonNumber ASC, EpisodeNumber ASC",
    )
    .bind(series_id)
    .fetch_all(pool)
    .await?;

    // Season node content-id, by season number, created lazily in walk order so
    // a season always precedes its episodes in `contents`.
    let mut season_nodes: HashMap<i64, ContentId> = HashMap::new();
    // The content index of each episode that a given EpisodeFileId satisfies, so
    // a multi-episode file links to all of them.
    let mut file_to_indices: HashMap<i64, Vec<usize>> = HashMap::new();

    for erow in &episode_rows {
        let season: i64 = erow.try_get("season").unwrap_or(0);
        let episode: i64 = erow.try_get("episode").unwrap_or(0);
        let absolute: Option<i64> = erow.try_get("absolute").ok().flatten();
        let ep_title = opt_text(erow, "title").unwrap_or_default();
        let monitored: i64 = erow.try_get("monitored").unwrap_or(0);
        let file_id: Option<i64> = erow.try_get("file_id").ok().flatten();

        let season_content_id = *season_nodes.entry(season).or_insert_with(|| {
            let id = ContentId::new();
            contents.push(MappedContent {
                node: ContentNode {
                    id,
                    library_id,
                    media_type: MediaType::Tv,
                    parent_id: Some(series_content_id),
                    kind: ContentKind::Season,
                    coords: Coordinates::Episode {
                        season: season.max(0) as u32,
                        episode: 0,
                        absolute: None,
                    },
                    // A season is monitored if any episode is; approximate with
                    // the series-level decision being inherited at episode level.
                    monitored: true,
                    title_id: None,
                },
                title: format!("{series_title} - Season {season}"),
                year: None,
                external_ids: ExternalIds::default(),
            });
            id
        });

        let content_index = contents.len();
        contents.push(MappedContent {
            node: ContentNode {
                id: ContentId::new(),
                library_id,
                media_type: MediaType::Tv,
                parent_id: Some(season_content_id),
                kind: ContentKind::Episode,
                coords: Coordinates::Episode {
                    season: season.max(0) as u32,
                    episode: episode.max(0) as u32,
                    absolute: absolute.filter(|a| *a > 0).map(|a| a as u32),
                },
                monitored: monitored != 0,
                title_id: None,
            },
            title: ep_title,
            year: None,
            external_ids: ExternalIds::default(),
        });

        if let Some(fid) = file_id.filter(|id| *id != 0) {
            file_to_indices.entry(fid).or_default().push(content_index);
        }
    }

    // Materialize each EpisodeFile once and link it to every episode it covers.
    for (fid, indices) in file_to_indices {
        if let Some(file) = read_episode_file(pool, fid, ranking).await? {
            files.push(MappedFile {
                file,
                content_indices: indices,
            });
        }
    }

    Ok(())
}

/// Read a single `EpisodeFiles` row by id.
async fn read_episode_file(
    pool: &SqlitePool,
    file_id: i64,
    ranking: &QualityRanking,
) -> Result<Option<MediaFile>> {
    let row = sqlx::query(
        "SELECT Id AS id, Path AS path, Size AS size, Quality AS quality
         FROM EpisodeFiles WHERE Id = ?1 LIMIT 1",
    )
    .bind(file_id)
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
        &format!("Sonarr EpisodeFile {file_id} Quality"),
    )?;

    Ok(Some(MediaFile {
        id: MediaFileId::new(),
        path,
        size: size.max(0) as u64,
        quality,
        languages: Vec::new(),
        media_info: None,
        custom_format_score: None,
    }))
}

/// Read Sonarr `QualityProfiles`.
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

/// Read Sonarr `CustomFormats`.
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
