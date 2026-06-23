//! Writing a [`MappedInstall`] into the destination cellarr database.
//!
//! Everything goes through `cellarr-db`'s public surface: the repository upserts
//! for the structural and config rows, and the shared writer-actor for the typed
//! identity side-tables (`movie_meta` / `series_meta` / `episode_meta`), which
//! preserve the external ids so cellarr never re-identifies the library. Those
//! side-tables have no repository yet, so we write them via the same single
//! writer every other mutation funnels through — never a second connection — so
//! the single-writer discipline (docs/08-database.md) is respected.

use cellarr_core::repo::ContentRepository;
use cellarr_core::{ContentId, ContentKind, MediaFileId, TitleId};
use cellarr_db::Database;

use crate::error::Result;
use crate::model::{ExternalIds, MappedContent, MappedInstall};

/// Persist the whole install. Returns the number of identity (external-id) rows
/// written.
pub(crate) async fn write_install(install: &MappedInstall, db: &Database) -> Result<usize> {
    write_profiles(install, db).await?;
    write_config(install, db).await?;
    write_libraries(install, db).await?;
    let identities = write_contents(install, db).await?;
    write_files(install, db).await?;
    Ok(identities)
}

/// Write quality profiles and custom formats.
async fn write_profiles(install: &MappedInstall, db: &Database) -> Result<()> {
    let profiles = db.profiles();
    for cf in &install.custom_formats {
        profiles.upsert_custom_format(cf).await?;
    }
    for p in &install.profiles {
        profiles.upsert_profile(p).await?;
    }
    Ok(())
}

/// Write indexers, download clients, and root folders.
async fn write_config(install: &MappedInstall, db: &Database) -> Result<()> {
    let config = db.config();
    for rf in &install.root_folders {
        config.upsert_root_folder(rf).await?;
    }
    for ix in &install.indexers {
        config.upsert_indexer(ix).await?;
    }
    for dc in &install.download_clients {
        config.upsert_download_client(dc).await?;
    }
    Ok(())
}

/// Write libraries.
async fn write_libraries(install: &MappedInstall, db: &Database) -> Result<()> {
    let config = db.config();
    for lib in &install.libraries {
        config.upsert_library(lib).await?;
    }
    Ok(())
}

/// Write content nodes (parents before children — the mapped order guarantees
/// it) plus their typed identity rows. Returns the identity-row count.
async fn write_contents(install: &MappedInstall, db: &Database) -> Result<usize> {
    let content_repo = db.content();
    let mut identities = 0;

    for mapped in &install.contents {
        // Attach a title_id to nodes that carry external identity, so the typed
        // *_meta row can be linked. Containers without ids (seasons) get none.
        let needs_identity = !mapped.external_ids.is_empty()
            || matches!(
                mapped.node.kind,
                ContentKind::Movie | ContentKind::Series | ContentKind::Episode
            );
        let title_id = needs_identity.then(TitleId::new);

        let mut node = mapped.node.clone();
        node.title_id = title_id;
        content_repo.upsert(&node).await?;

        // Index the display title for library search.
        if !mapped.title.is_empty() {
            content_repo.index_title(node.id, &mapped.title).await?;
        }

        if let Some(tid) = title_id {
            // Each typed metadata row is a preserved identity (title + any
            // external ids), so cellarr never re-identifies the item.
            write_meta(db, tid, mapped).await?;
            identities += 1;
        }
    }

    Ok(identities)
}

/// Write the typed identity/metadata side-row for a content node via the shared
/// writer-actor.
async fn write_meta(db: &Database, title_id: TitleId, mapped: &MappedContent) -> Result<()> {
    match mapped.node.kind {
        ContentKind::Movie => write_movie_meta(db, title_id, mapped).await,
        ContentKind::Series => write_series_meta(db, title_id, mapped).await,
        ContentKind::Episode => write_episode_meta(db, title_id, mapped).await,
        // Seasons and other container kinds have no typed identity table in the
        // current schema; their structure is already captured by the content row.
        _ => Ok(()),
    }
}

async fn write_movie_meta(db: &Database, title_id: TitleId, mapped: &MappedContent) -> Result<()> {
    let tid = title_id.to_string();
    let title = mapped.title.clone();
    let year = mapped.year;
    let ExternalIds {
        tmdb_id, imdb_id, ..
    } = mapped.external_ids.clone();
    db.writer()
        .submit(move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO movie_meta (title_id, title, year, tmdb_id, imdb_id)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(title_id) DO UPDATE SET
                        title = excluded.title, year = excluded.year,
                        tmdb_id = excluded.tmdb_id, imdb_id = excluded.imdb_id",
                )
                .bind(tid)
                .bind(title)
                .bind(year)
                .bind(tmdb_id)
                .bind(imdb_id)
                .execute(&mut *conn)
                .await?;
                Ok(())
            })
        })
        .await?;
    Ok(())
}

async fn write_series_meta(db: &Database, title_id: TitleId, mapped: &MappedContent) -> Result<()> {
    let tid = title_id.to_string();
    let title = mapped.title.clone();
    let year = mapped.year;
    let ExternalIds {
        tmdb_id,
        tvdb_id,
        imdb_id,
    } = mapped.external_ids.clone();
    db.writer()
        .submit(move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO series_meta (title_id, title, year, tvdb_id, tmdb_id, imdb_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(title_id) DO UPDATE SET
                        title = excluded.title, year = excluded.year,
                        tvdb_id = excluded.tvdb_id, tmdb_id = excluded.tmdb_id,
                        imdb_id = excluded.imdb_id",
                )
                .bind(tid)
                .bind(title)
                .bind(year)
                .bind(tvdb_id)
                .bind(tmdb_id)
                .bind(imdb_id)
                .execute(&mut *conn)
                .await?;
                Ok(())
            })
        })
        .await?;
    Ok(())
}

async fn write_episode_meta(
    db: &Database,
    title_id: TitleId,
    mapped: &MappedContent,
) -> Result<()> {
    // Episode coordinates carry the season/episode numbers the side-row needs.
    let (season, episode, absolute) = match &mapped.node.coords {
        cellarr_core::Coordinates::Episode {
            season,
            episode,
            absolute,
        } => (
            i64::from(*season),
            i64::from(*episode),
            absolute.map(i64::from),
        ),
        _ => (0, 0, None),
    };
    let tid = title_id.to_string();
    let title = mapped.title.clone();
    db.writer()
        .submit(move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO episode_meta
                        (title_id, season_number, episode_number, absolute_number, title)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(title_id) DO UPDATE SET
                        season_number = excluded.season_number,
                        episode_number = excluded.episode_number,
                        absolute_number = excluded.absolute_number,
                        title = excluded.title",
                )
                .bind(tid)
                .bind(season)
                .bind(episode)
                .bind(absolute)
                .bind(title)
                .execute(&mut *conn)
                .await?;
                Ok(())
            })
        })
        .await?;
    Ok(())
}

/// Write media files and their content links — recognized in place at their
/// existing paths.
async fn write_files(install: &MappedInstall, db: &Database) -> Result<()> {
    use cellarr_core::repo::MediaFileRepository;

    let media_repo = db.media_files();
    for mapped in &install.files {
        media_repo.create(&mapped.file).await?;
        let file_id: MediaFileId = mapped.file.id;
        for idx in &mapped.content_indices {
            if let Some(content) = install.contents.get(*idx) {
                let content_id: ContentId = content.node.id;
                media_repo.link(content_id, file_id).await?;
            }
        }
    }
    Ok(())
}
