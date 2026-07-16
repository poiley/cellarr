//! The structural `content` tree repository.

use std::sync::{Arc, Mutex};

use crate::dialect::{pq, DbPool};
use async_trait::async_trait;
use cellarr_core::repo::{ContentRepository, DeletedContent};
use cellarr_core::{
    ContentId, ContentKind, ContentMetadata, ContentNode, ContentRef, Coordinates, LibraryId,
    MediaType, SeriesType, TitleId,
};
use sqlx::Row;

use crate::convert::parse_uuid;
use crate::error::{DbError, Result};
use crate::writer::WriterHandle;

/// Reads/writes for the `content` adjacency list.
#[derive(Clone)]
pub struct ContentRepo {
    pool: DbPool,
    writer: WriterHandle,
}

impl ContentRepo {
    pub(crate) fn new(pool: DbPool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }

    /// The bounded set of QUALITY-UPGRADE candidates: monitored leaf nodes that
    /// already HAVE a linked media_file (so they are not "missing" — that is
    /// [`ContentRepository::monitored_missing`]'s job) and may have a better release
    /// available. Ordered least-recently-searched first — a node never searched
    /// (no `upgrade_search` row) sorts ahead of any that have — so successive runs
    /// rotate through the whole backlog instead of re-hammering the same head, then
    /// bounded to `limit` per run to protect the indexers. The decision engine still
    /// decides per node whether a real upgrade exists (respecting the profile
    /// cutoff); this only supplies the candidates.
    ///
    /// The `(searched_at IS NULL) DESC, searched_at ASC` ordering puts never-searched
    /// nodes first, then oldest-searched, identically on SQLite and Postgres (whose
    /// default NULL ordering differs).
    pub async fn upgrade_candidates(&self, limit: usize) -> Result<Vec<ContentRef>> {
        let rows = sqlx::query(&pq(
            "SELECT c.id, c.library_id, c.media_type, c.parent_id, c.kind, c.series_type, c.coords,
                    c.monitored, c.title_id
             FROM content c
             LEFT JOIN upgrade_search u ON u.content_id = c.id
             WHERE c.monitored = 1
               AND c.kind IN ('movie', 'episode', 'track', 'book')
               AND EXISTS (
                   SELECT 1 FROM content_file cf WHERE cf.content_id = c.id
               )
             ORDER BY (u.searched_at IS NULL) DESC, u.searched_at ASC
             LIMIT ?1",
        ))
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| row_to_node(r).map(|n| n.as_ref()))
            .collect()
    }

    /// Record that `ids` were just considered for an upgrade, so the next sweep
    /// moves on to the next least-recently-searched slice. Upserts each node's
    /// `searched_at` to now. Best-effort ordering bookkeeping — a write failure only
    /// costs fairness on the next run, never correctness.
    pub async fn mark_upgrade_searched(&self, ids: Vec<ContentId>) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let now = crate::convert::format_time(time::OffsetDateTime::now_utc())?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    for id in ids {
                        sqlx::query(&pq(
                            "INSERT INTO upgrade_search (content_id, searched_at)
                             VALUES (?1, ?2)
                             ON CONFLICT(content_id) DO UPDATE SET searched_at = excluded.searched_at",
                        ))
                        .bind(id.to_string())
                        .bind(&now)
                        .execute(&mut *conn)
                        .await?;
                    }
                    Ok(())
                })
            })
            .await
    }

    /// Record that `ids` were just run through an acquisition search, so the next
    /// monitored-missing sweep moves on to the next least-recently-searched slice.
    /// Upserts each node's `searched_at` to now. Best-effort ordering bookkeeping — a
    /// write failure only costs fairness on the next run, never correctness. The
    /// acquisition counterpart to [`mark_upgrade_searched`](Self::mark_upgrade_searched).
    pub async fn mark_missing_searched(&self, ids: Vec<ContentId>) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let now = crate::convert::format_time(time::OffsetDateTime::now_utc())?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    for id in ids {
                        sqlx::query(&pq(
                            "INSERT INTO missing_search (content_id, searched_at)
                             VALUES (?1, ?2)
                             ON CONFLICT(content_id) DO UPDATE SET searched_at = excluded.searched_at",
                        ))
                        .bind(id.to_string())
                        .bind(&now)
                        .execute(&mut *conn)
                        .await?;
                    }
                    Ok(())
                })
            })
            .await
    }

    /// The series content-node ids most in need of a metadata refresh — never-refreshed
    /// first (no `series_refresh` row), then least-recently-refreshed. Bounded to
    /// `limit` so a whole-library MetadataRefresh re-resolves a gentle batch per run
    /// instead of every series at once (which can saturate a small database). Over
    /// successive runs every series is covered.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query failure.
    pub async fn series_due_for_refresh(&self, limit: usize) -> Result<Vec<ContentId>> {
        let rows = sqlx::query(&pq(
            "SELECT c.id FROM content c
             LEFT JOIN series_refresh sr ON sr.content_id = c.id
             WHERE c.kind = 'series'
             ORDER BY (sr.resolved_at IS NULL) DESC, sr.resolved_at ASC
             LIMIT ?1",
        ))
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                let s: String = r.try_get("id")?;
                Ok(ContentId::from_uuid(parse_uuid("content_id", &s)?))
            })
            .collect()
    }

    /// Record that a series was just re-resolved, so the next refresh moves on to the
    /// next least-recently-refreshed batch. Upserts its `resolved_at` to now.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn mark_series_refreshed(&self, series: ContentId) -> Result<()> {
        let now = crate::convert::format_time(time::OffsetDateTime::now_utc())?;
        let id = series.to_string();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq(
                        "INSERT INTO series_refresh (content_id, resolved_at)
                         VALUES (?1, ?2)
                         ON CONFLICT(content_id) DO UPDATE SET resolved_at = excluded.resolved_at",
                    ))
                    .bind(&id)
                    .bind(&now)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Index (or re-index) a node's searchable title in the FTS table.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn index_title(&self, id: ContentId, title: &str) -> Result<()> {
        let id = id.to_string();
        let title = title.to_string();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq("DELETE FROM content_fts WHERE content_id = ?1"))
                        .bind(&id)
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query(&pq(
                        "INSERT INTO content_fts (content_id, title) VALUES (?1, ?2)",
                    ))
                    .bind(&id)
                    .bind(&title)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Full-text search content titles, returning matching node ids best-first.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query failure.
    pub async fn search(&self, query: &str) -> Result<Vec<ContentId>> {
        // Reduce the query to alphanumeric tokens before it reaches the engine. A
        // title carrying FTS operators or separators is otherwise unsearchable:
        // SQLite FTS5 reads `-` as an operator, so a raw `Obi-Wan Kenobi` MATCH
        // ERRORS ("no such column: Wan"); and both engines' tokenizers split
        // `Obi-Wan` / `11.22.63` into `obi wan` / `11 22 63`, so the query must
        // match on those same tokens. Punctuation-only input yields no tokens → no
        // rows (never a malformed MATCH). The index side matches: SQLite FTS5
        // already tokenizes on non-alphanumerics; Postgres normalizes the same way
        // in the `title_tsv` generated column (see the content_fts normalize migration).
        let tokens: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(str::to_lowercase)
            .collect();
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        // SQLite: each token as a quoted phrase literal so a stray FTS5 keyword or
        // operator character can never be interpreted (implicit AND across them).
        // Postgres: a plain space-joined string for `plainto_tsquery`.
        #[cfg(not(feature = "postgres"))]
        let bound = tokens
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(" ");
        #[cfg(feature = "postgres")]
        let bound = tokens.join(" ");

        // FTS is the one query whose shape genuinely differs between engines:
        // SQLite matches the FTS5 virtual table and orders by its `rank`; Postgres
        // matches the generated `tsvector` column and orders by `ts_rank`. Both
        // read a single bound parameter (`?1`, which `pq` renders as `$1` — reused
        // twice in the Postgres form, which Postgres permits for one bind).
        #[cfg(not(feature = "postgres"))]
        let sql = "SELECT content_id FROM content_fts WHERE content_fts MATCH ?1 ORDER BY rank";
        #[cfg(feature = "postgres")]
        let sql = "SELECT content_id FROM content_fts \
                   WHERE title_tsv @@ plainto_tsquery('simple', ?1) \
                   ORDER BY ts_rank(title_tsv, plainto_tsquery('simple', ?1)) DESC";
        let rows = sqlx::query(&pq(sql))
            .bind(&bound)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|r| {
                let s: String = r.try_get("content_id")?;
                Ok(ContentId::from_uuid(parse_uuid("content_id", &s)?))
            })
            .collect()
    }

    /// Recover the searchable title indexed for a node, if one was indexed.
    ///
    /// The `content` row carries no title column (titles live in the FTS index),
    /// so this is the reverse of [`index_title`](Self::index_title): it lets the
    /// list resources surface a node's real title instead of its UUID. `None`
    /// means the node has no indexed title (it was never identified/added with a
    /// title), and the caller falls back to the id.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query failure.
    pub async fn title_for(&self, id: ContentId) -> Result<Option<String>> {
        let row = sqlx::query(&pq("SELECT title FROM content_fts WHERE content_id = ?1"))
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| r.try_get::<String, _>("title").map_err(DbError::from))
            .transpose()
    }

    // --- Batched reads for the list projections -----------------------------
    //
    // The library list renders every root node; doing the per-node reads
    // (`title_for`/`metadata`/`external_id_for`) inside that loop is an N+1 that
    // fired thousands of queries for a large library. These fetch the whole set in
    // ONE query each so the list handler can assemble in memory.

    /// Every indexed title, keyed by content id.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn all_titles(&self) -> Result<std::collections::HashMap<ContentId, String>> {
        let rows = sqlx::query(&pq("SELECT content_id, title FROM content_fts"))
            .fetch_all(&self.pool)
            .await?;
        let mut map = std::collections::HashMap::with_capacity(rows.len());
        for r in rows {
            let id: String = r.try_get("content_id")?;
            let title: String = r.try_get("title")?;
            map.insert(ContentId::from_uuid(parse_uuid("content_id", &id)?), title);
        }
        Ok(map)
    }

    /// Every content-metadata year, keyed by content id (rows with no year skipped).
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn all_years(&self) -> Result<std::collections::HashMap<ContentId, u16>> {
        let rows = sqlx::query(&pq(
            "SELECT content_id, year FROM content_meta WHERE year IS NOT NULL",
        ))
        .fetch_all(&self.pool)
        .await?;
        let mut map = std::collections::HashMap::with_capacity(rows.len());
        for r in rows {
            let id: String = r.try_get("content_id")?;
            let year: Option<i64> = r.try_get("year")?;
            if let Some(y) = year {
                map.insert(
                    ContentId::from_uuid(parse_uuid("content_id", &id)?),
                    y as u16,
                );
            }
        }
        Ok(map)
    }

    /// Every node's native external id `(scheme, value)`, keyed by content id — the
    /// bulk form of [`external_id_for`](Self::external_id_for), preferring the
    /// namespace the media type keys on (tvdb for TV, tmdb for movies), then imdb.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn all_external_ids(
        &self,
        media_type: MediaType,
    ) -> Result<std::collections::HashMap<ContentId, (String, String)>> {
        let sql = match media_type {
            MediaType::Movie => {
                "SELECT c.id AS id, m.tmdb_id AS tmdb_id, NULL AS tvdb_id, m.imdb_id AS imdb_id
                 FROM content c JOIN movie_meta m ON m.title_id = c.title_id"
            }
            MediaType::Tv => {
                "SELECT c.id AS id, s.tmdb_id AS tmdb_id, s.tvdb_id AS tvdb_id, s.imdb_id AS imdb_id
                 FROM content c JOIN series_meta s ON s.title_id = c.title_id"
            }
            MediaType::Music | MediaType::Book => return Ok(std::collections::HashMap::new()),
        };
        let rows = sqlx::query(&pq(sql)).fetch_all(&self.pool).await?;
        let mut map = std::collections::HashMap::with_capacity(rows.len());
        for r in rows {
            let id: String = r.try_get("id")?;
            let tmdb: Option<i64> = r.try_get("tmdb_id").unwrap_or(None);
            let tvdb: Option<i64> = r.try_get("tvdb_id").unwrap_or(None);
            let imdb: Option<String> = r.try_get("imdb_id").unwrap_or(None);
            let picked = if media_type == MediaType::Tv {
                tvdb.map(|v| ("tvdb".to_string(), v.to_string()))
            } else {
                None
            }
            .or_else(|| tmdb.map(|v| ("tmdb".to_string(), v.to_string())))
            .or_else(|| {
                imdb.filter(|v| !v.trim().is_empty())
                    .map(|v| ("imdb".to_string(), v.trim().to_string()))
            });
            if let Some(pair) = picked {
                map.insert(ContentId::from_uuid(parse_uuid("id", &id)?), pair);
            }
        }
        Ok(map)
    }

    /// Persist an external id (`id_type` + `id_value`) for a content node, the way
    /// the identify pipeline does: mint a `title_id`, link it onto the node, and
    /// write the matching typed `*_meta` identity row carrying the external id.
    ///
    /// This is what lets an import-list-added node carry its `tmdb`/`tvdb`/`imdb`
    /// id, so re-syncing the same list dedups against it (via
    /// [`external_keys`](Self::external_keys)) instead of adding a duplicate, and so
    /// the v3 projection surfaces a real `tmdbId`/`tvdbId` instead of `0`.
    ///
    /// Only the `movie`/`series` identity tables are written (the kinds import
    /// lists add as roots); other kinds carry their identity elsewhere and are a
    /// no-op here. The whole link is one writer transaction so a crash can never
    /// leave a node pointing at a missing `*_meta` row.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn link_external_id(
        &self,
        id: ContentId,
        media_type: MediaType,
        id_type: &str,
        id_value: &str,
        title: &str,
    ) -> Result<()> {
        // Reuse any title_id already linked to the node so a re-link updates the
        // existing identity row in place rather than orphaning the old one.
        let existing_title_id: Option<String> =
            sqlx::query(&pq("SELECT title_id FROM content WHERE id = ?1"))
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await?
                .and_then(|r| r.try_get::<Option<String>, _>("title_id").ok().flatten());
        let title_id = existing_title_id.unwrap_or_else(|| TitleId::new().to_string());

        // The numeric id sources (tmdb/tvdb) store an INTEGER; imdb is a TEXT id.
        let key = id_type.trim().to_ascii_lowercase();
        let numeric_id: Option<i64> = id_value.trim().parse::<i64>().ok();
        let imdb_id: Option<String> = (key == "imdb").then(|| id_value.trim().to_string());

        let content_id = id.to_string();
        let title = title.to_string();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    // Link the title_id onto the node.
                    sqlx::query(&pq("UPDATE content SET title_id = ?1 WHERE id = ?2"))
                        .bind(&title_id)
                        .bind(&content_id)
                        .execute(&mut *conn)
                        .await?;

                    match media_type {
                        MediaType::Movie => {
                            let (tmdb_id, imdb) = if key == "imdb" {
                                (None, imdb_id.clone())
                            } else {
                                (numeric_id, None)
                            };
                            sqlx::query(&pq(
                                "INSERT INTO movie_meta (title_id, title, tmdb_id, imdb_id)
                                 VALUES (?1, ?2, ?3, ?4)
                                 ON CONFLICT(title_id) DO UPDATE SET
                                    title = excluded.title,
                                    tmdb_id = COALESCE(excluded.tmdb_id, movie_meta.tmdb_id),
                                    imdb_id = COALESCE(excluded.imdb_id, movie_meta.imdb_id)"),
                            )
                            .bind(&title_id)
                            .bind(&title)
                            .bind(tmdb_id)
                            .bind(imdb)
                            .execute(&mut *conn)
                            .await?;
                        }
                        MediaType::Tv => {
                            let (tvdb_id, tmdb_id, imdb) = match key.as_str() {
                                "tvdb" => (numeric_id, None, None),
                                "imdb" => (None, None, imdb_id.clone()),
                                _ => (None, numeric_id, None),
                            };
                            sqlx::query(&pq(
                                "INSERT INTO series_meta (title_id, title, tvdb_id, tmdb_id, imdb_id)
                                 VALUES (?1, ?2, ?3, ?4, ?5)
                                 ON CONFLICT(title_id) DO UPDATE SET
                                    title = excluded.title,
                                    tvdb_id = COALESCE(excluded.tvdb_id, series_meta.tvdb_id),
                                    tmdb_id = COALESCE(excluded.tmdb_id, series_meta.tmdb_id),
                                    imdb_id = COALESCE(excluded.imdb_id, series_meta.imdb_id)"),
                            )
                            .bind(&title_id)
                            .bind(&title)
                            .bind(tvdb_id)
                            .bind(tmdb_id)
                            .bind(imdb)
                            .execute(&mut *conn)
                            .await?;
                        }
                        // Music/book identity tables are deferred (see migration 0001);
                        // the node's title_id link is still written above so a future
                        // identity table can attach without re-keying.
                        MediaType::Music | MediaType::Book => {}
                    }
                    Ok(())
                })
            })
            .await
    }

    /// Persist a series' alternate titles (from the metadata source) onto its
    /// `series_meta` row, keyed via the series node's `title_id`. Stored as a JSON
    /// array so the content matcher can accept a file whose parsed title matches an
    /// alias rather than the canonical (possibly non-English) title.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    /// Reconcile a movie/series node's **identity title** with the authoritative
    /// title from a metadata resolve.
    ///
    /// The identity row (`movie_meta`/`series_meta`) is first seeded at create time
    /// from the *search-result* title, which for a non-English work is often the
    /// provider's native primary (anime titled `キルラキル`). The details fetch then
    /// yields the display title cellarr should use (`normalize_series` prefers a
    /// Latin one). Without this, the search title is frozen in place and the v3
    /// projection shows the native name forever. Called on every resolve (create +
    /// refresh) so the title tracks the provider, exactly as Sonarr/Radarr refresh.
    ///
    /// A no-op for kinds without an identity table (music/book) and when the node
    /// carries no `title_id` yet.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn set_identity_title(
        &self,
        content: ContentId,
        media_type: MediaType,
        title: &str,
    ) -> Result<()> {
        let table = match media_type {
            MediaType::Movie => "movie_meta",
            MediaType::Tv => "series_meta",
            MediaType::Music | MediaType::Book => return Ok(()),
        };
        let id = content.to_string();
        let title = title.to_string();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq(&format!(
                        "UPDATE {table} SET title = ?2
                         WHERE title_id = (SELECT title_id FROM content WHERE id = ?1)"
                    )))
                    .bind(&id)
                    .bind(&title)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    pub async fn set_series_aliases(&self, series: ContentId, aliases: &[String]) -> Result<()> {
        let json = serde_json::to_string(aliases)?;
        let id = series.to_string();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq(
                        "UPDATE series_meta SET aliases = ?2
                         WHERE title_id = (SELECT title_id FROM content WHERE id = ?1)",
                    ))
                    .bind(&id)
                    .bind(&json)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// The alternate titles of the SERIES a content node belongs to — used by the
    /// matcher so an episode candidate carries its series' aliases. Walks up the
    /// adjacency list to the node that carries a `title_id` (the series root;
    /// seasons/episodes carry none), then reads that series' stored aliases. Empty
    /// when none are stored (or the node is not under an identified series).
    ///
    /// # Errors
    /// Returns a [`DbError`] on query failure.
    pub async fn aliases_for_content(&self, id: ContentId) -> Result<Vec<String>> {
        let mut cur = Some(id.to_string());
        let mut title_id: Option<String> = None;
        // Bounded walk: series → season → episode is three levels; cap well above.
        for _ in 0..8 {
            let Some(c) = cur.take() else { break };
            let Some(row) = sqlx::query(&pq(
                "SELECT parent_id, title_id FROM content WHERE id = ?1",
            ))
            .bind(&c)
            .fetch_optional(&self.pool)
            .await?
            else {
                break;
            };
            if let Some(tid) = row.try_get::<Option<String>, _>("title_id")? {
                title_id = Some(tid);
                break;
            }
            cur = row.try_get::<Option<String>, _>("parent_id")?;
        }
        let Some(tid) = title_id else {
            return Ok(Vec::new());
        };
        let json: Option<String> = sqlx::query(&pq(
            "SELECT aliases FROM series_meta WHERE title_id = ?1",
        ))
        .bind(&tid)
        .fetch_optional(&self.pool)
        .await?
        .and_then(|r| r.try_get::<Option<String>, _>("aliases").ok().flatten());
        Ok(json
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .unwrap_or_default())
    }

    /// The set of external-id identity keys `(id_type, id_value)` already present
    /// for `media_type`, read back from the typed `*_meta` identity rows linked to
    /// this library's content nodes.
    ///
    /// This is the read side of [`link_external_id`](Self::link_external_id): it is
    /// what the import-list sync's `existing_keys` resolves so a re-sync of the same
    /// list skips items already in the library (idempotency). Keys are normalized
    /// (lowercased namespace) to match [`ImportListItem::key`]. A node with no
    /// linked identity contributes nothing (it is simply not yet de-dupable).
    ///
    /// # Errors
    /// Returns a [`DbError`] on query failure.
    pub async fn external_keys(&self, media_type: MediaType) -> Result<Vec<(String, String)>> {
        // Only movie/series carry typed external-id tables today; the music/book
        // identity tables are deferred, so those media types have nothing to read
        // back yet (returning an empty set is safe — it only relaxes de-dup).
        let rows = match media_type {
            MediaType::Movie => {
                sqlx::query(&pq("SELECT m.tmdb_id AS tmdb_id, m.imdb_id AS imdb_id
                     FROM content c JOIN movie_meta m ON m.title_id = c.title_id
                     WHERE c.media_type = 'movie'"))
                .fetch_all(&self.pool)
                .await?
            }
            MediaType::Tv => {
                sqlx::query(&pq(
                    "SELECT s.tmdb_id AS tmdb_id, s.imdb_id AS imdb_id, s.tvdb_id AS tvdb_id
                     FROM content c JOIN series_meta s ON s.title_id = c.title_id
                     WHERE c.media_type = 'tv'",
                ))
                .fetch_all(&self.pool)
                .await?
            }
            MediaType::Music | MediaType::Book => Vec::new(),
        };

        let mut keys = Vec::new();
        for row in &rows {
            if let Ok(Some(tmdb)) = row.try_get::<Option<i64>, _>("tmdb_id") {
                keys.push(("tmdb".to_string(), tmdb.to_string()));
            }
            if let Ok(Some(imdb)) = row.try_get::<Option<String>, _>("imdb_id") {
                let v = imdb.trim().to_ascii_lowercase();
                if !v.is_empty() {
                    keys.push(("imdb".to_string(), v));
                }
            }
            if media_type == MediaType::Tv {
                if let Ok(Some(tvdb)) = row.try_get::<Option<i64>, _>("tvdb_id") {
                    keys.push(("tvdb".to_string(), tvdb.to_string()));
                }
            }
        }
        Ok(keys)
    }

    /// Read back the primary external id for a single content node as
    /// `(id_type, id_value)` — the `tmdb`/`tvdb`/`imdb` id the v3 projection
    /// surfaces. `None` when the node carries no linked identity row yet.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn external_id_for(
        &self,
        id: ContentId,
        media_type: MediaType,
    ) -> Result<Option<(String, String)>> {
        let row = match media_type {
            MediaType::Movie => {
                sqlx::query(&pq("SELECT m.tmdb_id AS tmdb_id, m.imdb_id AS imdb_id
                 FROM content c JOIN movie_meta m ON m.title_id = c.title_id
                 WHERE c.id = ?1"))
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await?
                .map(|r| {
                    let tmdb: Option<i64> = r.try_get("tmdb_id").unwrap_or(None);
                    let imdb: Option<String> = r.try_get("imdb_id").unwrap_or(None);
                    (tmdb, None::<i64>, imdb)
                })
            }
            MediaType::Tv => sqlx::query(&pq(
                "SELECT s.tvdb_id AS tvdb_id, s.tmdb_id AS tmdb_id, s.imdb_id AS imdb_id
                 FROM content c JOIN series_meta s ON s.title_id = c.title_id
                 WHERE c.id = ?1",
            ))
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .map(|r| {
                let tvdb: Option<i64> = r.try_get("tvdb_id").unwrap_or(None);
                let tmdb: Option<i64> = r.try_get("tmdb_id").unwrap_or(None);
                let imdb: Option<String> = r.try_get("imdb_id").unwrap_or(None);
                (tmdb, tvdb, imdb)
            }),
            MediaType::Music | MediaType::Book => None,
        };
        let Some((tmdb, tvdb, imdb)) = row else {
            return Ok(None);
        };
        // Prefer the namespace the media type primarily keys on (tvdb for TV, tmdb
        // for movies), then fall back to whatever id is present.
        if media_type == MediaType::Tv {
            if let Some(v) = tvdb {
                return Ok(Some(("tvdb".to_string(), v.to_string())));
            }
        }
        if let Some(v) = tmdb {
            return Ok(Some(("tmdb".to_string(), v.to_string())));
        }
        if let Some(v) = imdb {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Ok(Some(("imdb".to_string(), v)));
            }
        }
        Ok(None)
    }

    /// The reverse of [`external_id_for`](Self::external_id_for): find the content
    /// node already carrying `(scheme, value)` for `media_type`, if any.
    ///
    /// The add path uses this to stay idempotent on identity — adding a title that
    /// is already in the library returns the existing node instead of creating a
    /// duplicate. `scheme` is `tmdb`/`tvdb`/`imdb`; a value that does not parse for a
    /// numeric scheme, or a media type with no external identity, yields `None`.
    pub async fn content_id_for_external_id(
        &self,
        media_type: MediaType,
        scheme: &str,
        value: &str,
    ) -> Result<Option<ContentId>> {
        let scheme = scheme.trim().to_ascii_lowercase();
        let value = value.trim();
        let numeric: Option<i64> = value.parse::<i64>().ok();
        let row = match (media_type, scheme.as_str()) {
            (MediaType::Movie, "tmdb") => match numeric {
                Some(n) => {
                    sqlx::query(&pq("SELECT c.id AS id FROM content c
                         JOIN movie_meta m ON m.title_id = c.title_id
                         WHERE m.tmdb_id = ?1 LIMIT 1"))
                    .bind(n)
                    .fetch_optional(&self.pool)
                    .await?
                }
                None => None,
            },
            (MediaType::Movie, "imdb") => {
                sqlx::query(&pq("SELECT c.id AS id FROM content c
                     JOIN movie_meta m ON m.title_id = c.title_id
                     WHERE m.imdb_id = ?1 LIMIT 1"))
                .bind(value)
                .fetch_optional(&self.pool)
                .await?
            }
            (MediaType::Tv, "tvdb") => match numeric {
                Some(n) => {
                    sqlx::query(&pq("SELECT c.id AS id FROM content c
                         JOIN series_meta s ON s.title_id = c.title_id
                         WHERE s.tvdb_id = ?1 LIMIT 1"))
                    .bind(n)
                    .fetch_optional(&self.pool)
                    .await?
                }
                None => None,
            },
            (MediaType::Tv, "tmdb") => match numeric {
                Some(n) => {
                    sqlx::query(&pq("SELECT c.id AS id FROM content c
                         JOIN series_meta s ON s.title_id = c.title_id
                         WHERE s.tmdb_id = ?1 LIMIT 1"))
                    .bind(n)
                    .fetch_optional(&self.pool)
                    .await?
                }
                None => None,
            },
            _ => None,
        };
        row.map(|r| {
            let id: String = r.try_get("id")?;
            Ok(ContentId::from_uuid(parse_uuid("id", &id)?))
        })
        .transpose()
    }

    /// Resolve a content node to the **TVDB id of the series it belongs to**.
    ///
    /// This is the identity-link query the anime absolute→episode remap is gated
    /// on: Identify needs the series' external id to select the right scene
    /// mapping. It walks the structural tree up from `id` to the series root
    /// (following `parent_id`), reads that node's `title_id`, and looks up
    /// `series_meta.tvdb_id` for it.
    ///
    /// Returns `None` when the node has no series ancestor, the series is not yet
    /// identity-linked (`title_id` is null), or the linked `series_meta` carries
    /// no `tvdb_id`. A `None` here means "identity unresolved" — the caller
    /// surfaces the absolute number for manual resolution rather than guessing
    /// (the library-safety rule), never an error.
    ///
    /// The walk is bounded by a depth cap so a malformed cycle in the adjacency
    /// list can never spin forever (the TV tree is at most series→season→episode,
    /// so a handful of hops suffices).
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn series_tvdb_id(&self, id: ContentId) -> Result<Option<i64>> {
        // Walk to the root of this node's tree. The series node is the root of a
        // TV content tree (series→season→episode); a depth cap guards against a
        // malformed parent cycle.
        const MAX_DEPTH: usize = 8;
        let mut current = id;
        let mut title_id: Option<String> = None;
        for _ in 0..MAX_DEPTH {
            let row = sqlx::query(&pq("SELECT parent_id, title_id FROM content WHERE id = ?1"))
                .bind(current.to_string())
                .fetch_optional(&self.pool)
                .await?;
            let Some(row) = row else {
                // The node (or a parent link) does not exist; unresolved.
                return Ok(None);
            };
            let parent_id: Option<String> = row.try_get("parent_id")?;
            let node_title_id: Option<String> = row.try_get("title_id")?;
            match parent_id {
                Some(parent) => {
                    // Not the root yet; keep the deepest title_id we have seen as a
                    // fallback but prefer the root series node's link.
                    current = ContentId::from_uuid(parse_uuid("parent_id", &parent)?);
                }
                None => {
                    // Reached the series root; its title_id is the series identity.
                    title_id = node_title_id;
                    break;
                }
            }
        }

        let Some(title_id) = title_id else {
            return Ok(None);
        };

        let row = sqlx::query(&pq("SELECT tvdb_id FROM series_meta WHERE title_id = ?1"))
            .bind(title_id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => Ok(row.try_get::<Option<i64>, _>("tvdb_id")?),
            None => Ok(None),
        }
    }

    /// Resolve a content node to the **[`SeriesType`] of the series it belongs
    /// to** — the gate that decides whether the anime absolute→episode remap runs
    /// for this node.
    ///
    /// Like [`series_tvdb_id`](Self::series_tvdb_id), it walks up the structural
    /// tree from `id` to the series root (following `parent_id`) and reads that
    /// root node's `series_type`, so an *episode* node correctly inherits its
    /// series' type. A node with no series ancestor (or a missing row) reads as
    /// [`SeriesType::Standard`] — the safe default that leaves an absolute number
    /// un-remapped rather than guessing, preserving prior behaviour.
    ///
    /// The walk is depth-capped against a malformed adjacency cycle (the TV tree
    /// is at most series→season→episode).
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn series_type_for(&self, id: ContentId) -> Result<SeriesType> {
        const MAX_DEPTH: usize = 8;
        let mut current = id;
        for _ in 0..MAX_DEPTH {
            let row = sqlx::query(&pq(
                "SELECT parent_id, series_type FROM content WHERE id = ?1",
            ))
            .bind(current.to_string())
            .fetch_optional(&self.pool)
            .await?;
            let Some(row) = row else {
                return Ok(SeriesType::Standard);
            };
            let parent_id: Option<String> = row.try_get("parent_id")?;
            let series_type: String = row.try_get("series_type")?;
            match parent_id {
                // Not the root yet; keep climbing to the series node, which is the
                // authoritative carrier of the type.
                Some(parent) => {
                    current = ContentId::from_uuid(parse_uuid("parent_id", &parent)?);
                }
                // Reached the series root: its series_type is the answer.
                None => return Ok(series_type_from_str(&series_type)),
            }
        }
        Ok(SeriesType::Standard)
    }

    /// Read the persisted content-scoped metadata for a node, or `None` when the
    /// node has never been identified/refreshed.
    ///
    /// This is the inherent twin of the [`ContentRepository::metadata`] trait
    /// method (which delegates here): it lets in-crate and sibling-crate callers
    /// — the registry's metadata seam, the detail endpoints — read the node's
    /// real facts (`title`/`year`/…) without first importing the repository
    /// trait. A `None` means the node carries no `content_meta` row yet (its
    /// year/overview are unknown), and the caller degrades gracefully rather than
    /// fabricating facts.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn metadata(&self, id: ContentId) -> Result<Option<ContentMetadata>> {
        let row = sqlx::query(&pq(
            "SELECT title, year, overview, runtime, air_date, digital_date,
                    genres, rating, rating_votes
             FROM content_meta WHERE content_id = ?1",
        ))
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_content_metadata).transpose()
    }

    /// Fetch a full [`ContentNode`] (not just the [`ContentRef`]), with its tag
    /// ids populated from the `content_tag` association.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn get_node(&self, id: ContentId) -> Result<Option<ContentNode>> {
        let row = sqlx::query(&pq(
            "SELECT id, library_id, media_type, parent_id, kind, series_type, coords, monitored, title_id
             FROM content WHERE id = ?1"),
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        let Some(mut node) = row.map(row_to_node).transpose()? else {
            return Ok(None);
        };
        node.tags = self.get_tags(id).await?;
        Ok(Some(node))
    }

    /// The tag ids associated with a content node, ascending. Empty when the node
    /// carries no tags (or does not exist).
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn get_tags(&self, id: ContentId) -> Result<Vec<u32>> {
        let rows = sqlx::query(&pq(
            "SELECT tag_id FROM content_tag WHERE content_id = ?1 ORDER BY tag_id ASC",
        ))
        .bind(id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                let v: i64 = r.try_get("tag_id")?;
                Ok(u32::try_from(v).unwrap_or(0))
            })
            .collect()
    }

    /// Replace the tag ids associated with a content node (the whole set is
    /// rewritten, so an empty `tags` clears them). One writer transaction so a
    /// crash never leaves a half-applied tag set.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn set_tags(&self, id: ContentId, tags: &[u32]) -> Result<()> {
        let content_id = id.to_string();
        // De-dup while preserving determinism (ascending).
        let mut ids: Vec<i64> = tags.iter().map(|t| i64::from(*t)).collect();
        ids.sort_unstable();
        ids.dedup();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq("DELETE FROM content_tag WHERE content_id = ?1"))
                        .bind(&content_id)
                        .execute(&mut *conn)
                        .await?;
                    for tag_id in &ids {
                        sqlx::query(&pq("INSERT INTO content_tag (content_id, tag_id)
                             VALUES (?1, ?2)
                             ON CONFLICT (content_id, tag_id) DO NOTHING"))
                        .bind(&content_id)
                        .bind(tag_id)
                        .execute(&mut *conn)
                        .await?;
                    }
                    Ok(())
                })
            })
            .await
    }

    /// Delete a content node identified by `id`, but only when its `kind` matches
    /// `expected_kind`. Returns the [`DeletedContent`] receipt, or `None` when the
    /// node does not exist or is the wrong kind (so the caller can 404 the
    /// addressed surface). Deletes the whole subtree (a series → its
    /// season/episode descendants), the orphaned `media_file` rows, the FTS index
    /// rows, and the node's history — all in **one** transaction so a crash can
    /// never leave the library half-deleted.
    ///
    /// `content_file` and `content_meta` rows fall away via `ON DELETE CASCADE`
    /// when the content node is removed; the virtual FTS table and `media_file`
    /// (referenced *by* `content_file`, so not reached by the content cascade) are
    /// cleaned explicitly here.
    async fn delete_subtree(
        &self,
        id: ContentId,
        expected_kind: ContentKind,
    ) -> Result<Option<DeletedContent>> {
        let want_kind = kind_to_str(expected_kind)?;
        let id_str = id.to_string();
        // The receipt is filled inside the write transaction and read back out
        // after it commits (the writer job returns `()` on success).
        let receipt: Arc<Mutex<Option<DeletedContent>>> = Arc::new(Mutex::new(None));
        let receipt_inner = Arc::clone(&receipt);

        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    // Guard the kind first: a wrong-kind / missing node deletes
                    // nothing and leaves an empty receipt → the caller 404s.
                    let row = sqlx::query(&pq("SELECT kind FROM content WHERE id = ?1"))
                        .bind(&id_str)
                        .fetch_optional(&mut *conn)
                        .await?;
                    let Some(row) = row else { return Ok(()) };
                    let kind: String = row.try_get("kind")?;
                    if kind != want_kind {
                        return Ok(());
                    }

                    // 1. Collect the whole subtree (root + descendants) by walking
                    //    the adjacency list breadth-first. A depth/size bound is
                    //    implicit: every id is visited once (we never revisit), so
                    //    even a malformed cycle terminates.
                    let mut ids: Vec<String> = vec![id_str.clone()];
                    let mut frontier: Vec<String> = vec![id_str.clone()];
                    while let Some(parent) = frontier.pop() {
                        let children =
                            sqlx::query(&pq("SELECT id FROM content WHERE parent_id = ?1"))
                                .bind(&parent)
                                .fetch_all(&mut *conn)
                                .await?;
                        for c in children {
                            let cid: String = c.try_get("id")?;
                            if !ids.contains(&cid) {
                                ids.push(cid.clone());
                                frontier.push(cid);
                            }
                        }
                    }

                    // 1b. The typed identity rows (series_meta / movie_meta, keyed by
                    //    title_id — NOT content_id, so no cascade reaches them). Collect
                    //    each subtree node's title_id now, while the rows still exist,
                    //    and delete the identities below; else a rolled-back onboard (or
                    //    any series/movie delete) leaves an orphan series_meta row.
                    let mut title_ids: Vec<String> = Vec::new();
                    for cid in &ids {
                        let row = sqlx::query(&pq("SELECT title_id FROM content WHERE id = ?1"))
                            .bind(cid)
                            .fetch_optional(&mut *conn)
                            .await?;
                        if let Some(tid) =
                            row.and_then(|r| r.try_get::<Option<String>, _>("title_id").ok().flatten())
                        {
                            if !title_ids.contains(&tid) {
                                title_ids.push(tid);
                            }
                        }
                    }

                    // 2. The media files linked anywhere under the subtree, and
                    //    their on-disk paths (the receipt the file step recycles).
                    let mut media_ids: Vec<String> = Vec::new();
                    let mut media_paths: Vec<String> = Vec::new();
                    for cid in &ids {
                        let rows = sqlx::query(&pq("SELECT mf.id AS id, mf.path AS path
                             FROM content_file cf
                             JOIN media_file mf ON mf.id = cf.media_file_id
                             WHERE cf.content_id = ?1"))
                        .bind(cid)
                        .fetch_all(&mut *conn)
                        .await?;
                        for r in rows {
                            let mid: String = r.try_get("id")?;
                            if !media_ids.contains(&mid) {
                                media_ids.push(mid);
                                media_paths.push(r.try_get("path")?);
                            }
                        }
                    }

                    // 3. Clean the rows the content cascade does NOT reach: the FTS
                    //    virtual table, the per-node history, and any grab whose
                    //    JSON content_ref targets a removed node.
                    for cid in &ids {
                        sqlx::query(&pq("DELETE FROM content_fts WHERE content_id = ?1"))
                            .bind(cid)
                            .execute(&mut *conn)
                            .await?;
                        sqlx::query(&pq("DELETE FROM history WHERE content_id = ?1"))
                            .bind(cid)
                            .execute(&mut *conn)
                            .await?;
                        // `content_ref` is TEXT holding a JSON ContentRef; extract
                        // its `id` field. SQLite's json_extract has no Postgres
                        // twin, so cast to jsonb and use `->>` there.
                        #[cfg(not(feature = "postgres"))]
                        let del_grab =
                            "DELETE FROM grab WHERE json_extract(content_ref, '$.id') = ?1";
                        #[cfg(feature = "postgres")]
                        let del_grab = "DELETE FROM grab WHERE (content_ref::jsonb ->> 'id') = ?1";
                        sqlx::query(&pq(del_grab))
                            .bind(cid)
                            .execute(&mut *conn)
                            .await?;
                    }

                    // 4. Remove the root node. `parent_id ... ON DELETE CASCADE`
                    //    takes the descendants, `content_file`, and `content_meta`
                    //    with it.
                    sqlx::query(&pq("DELETE FROM content WHERE id = ?1"))
                        .bind(&id_str)
                        .execute(&mut *conn)
                        .await?;

                    // 5. The media_file rows are referenced *by* content_file (the
                    //    cascade runs the other way), so removing the content does
                    //    not touch them. Delete the now-orphaned files explicitly.
                    for mid in &media_ids {
                        sqlx::query(&pq("DELETE FROM media_file WHERE id = ?1"))
                            .bind(mid)
                            .execute(&mut *conn)
                            .await?;
                    }

                    // 6. The typed identity rows keyed by title_id. season_meta /
                    //    episode_meta cascade from series_meta, so deleting the series /
                    //    movie identity clears the whole typed tree.
                    for tid in &title_ids {
                        sqlx::query(&pq("DELETE FROM series_meta WHERE title_id = ?1"))
                            .bind(tid)
                            .execute(&mut *conn)
                            .await?;
                        sqlx::query(&pq("DELETE FROM movie_meta WHERE title_id = ?1"))
                            .bind(tid)
                            .execute(&mut *conn)
                            .await?;
                    }

                    let content_ids = ids
                        .iter()
                        .map(|s| parse_uuid("content_id", s).map(ContentId::from_uuid))
                        .collect::<Result<Vec<_>>>()?;
                    *receipt_inner.lock().expect("receipt mutex poisoned") = Some(DeletedContent {
                        content_ids,
                        media_file_paths: media_paths,
                    });
                    Ok(())
                })
            })
            .await?;

        let out = receipt.lock().expect("receipt mutex poisoned").take();
        Ok(out)
    }
}

/// Serialize a [`ContentKind`] to its stored lowercase string form.
///
/// `ContentKind` serializes to a bare JSON string (`"episode"`); we want the raw
/// scalar for the `content.kind` TEXT column, so unwrap the JSON string. The
/// `unwrap_or_default` can never actually fire (the enum always serializes to a
/// string), but avoids a panic on the fallible runtime path.
fn kind_to_str(kind: ContentKind) -> Result<String> {
    Ok(serde_json::to_value(kind)?
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// Parse a stored `content.kind` string back into a [`ContentKind`].
fn kind_from_str(kind: &str) -> Result<ContentKind> {
    serde_json::from_value(serde_json::Value::String(kind.to_string())).map_err(DbError::from)
}

/// Serialize a [`SeriesType`] to its stored lowercase string form (the
/// `content.series_type` TEXT column).
fn series_type_to_str(series_type: SeriesType) -> String {
    serde_json::to_value(series_type)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "standard".to_string())
}

/// Parse a stored `content.series_type` string back into a [`SeriesType`].
///
/// An unrecognized/legacy value (or a row written before the column existed,
/// which reads as the `'standard'` default) decodes to [`SeriesType::Standard`]
/// rather than erroring, so the read path stays total and behaviour-preserving.
fn series_type_from_str(series_type: &str) -> SeriesType {
    serde_json::from_value(serde_json::Value::String(series_type.to_string()))
        .unwrap_or(SeriesType::Standard)
}

fn row_to_node(row: crate::dialect::DbRow) -> Result<ContentNode> {
    let id: String = row.try_get("id")?;
    let library_id: String = row.try_get("library_id")?;
    let media_type: String = row.try_get("media_type")?;
    let parent_id: Option<String> = row.try_get("parent_id")?;
    let kind: String = row.try_get("kind")?;
    let series_type: String = row.try_get("series_type")?;
    let coords: String = row.try_get("coords")?;
    let monitored: i64 = row.try_get("monitored")?;
    let title_id: Option<String> = row.try_get("title_id")?;

    let media_type: MediaType =
        serde_json::from_value(serde_json::Value::String(media_type)).map_err(DbError::from)?;
    let kind = kind_from_str(&kind)?;
    let series_type = series_type_from_str(&series_type);
    let coords: Coordinates = serde_json::from_str(&coords)?;
    let parent_id = parent_id
        .map(|p| parse_uuid("parent_id", &p).map(ContentId::from_uuid))
        .transpose()?;
    let title_id = title_id
        .map(|t| parse_uuid("title_id", &t).map(TitleId::from_uuid))
        .transpose()?;

    Ok(ContentNode {
        id: ContentId::from_uuid(parse_uuid("id", &id)?),
        library_id: LibraryId::from_uuid(parse_uuid("library_id", &library_id)?),
        media_type,
        parent_id,
        kind,
        series_type,
        coords,
        monitored: monitored != 0,
        title_id,
        // Tags live in the `content_tag` association, not on the row; the read
        // paths that need them populate this via `load_tags`. A bare row decode
        // leaves it empty.
        tags: Vec::new(),
    })
}

#[async_trait]
impl ContentRepository for ContentRepo {
    type Error = DbError;

    async fn get(&self, id: ContentId) -> Result<Option<ContentRef>> {
        Ok(self.get_node(id).await?.map(|n| n.as_ref()))
    }

    async fn monitored_missing(&self) -> Result<Vec<ContentRef>> {
        // Monitored nodes with no linked media_file are "missing". Containers
        // (series/season/artist/album/author) are excluded: only leaf, grabbable
        // nodes are acquisition targets.
        //
        // Ordered LEAST-RECENTLY-SEARCHED first: never-searched nodes (no
        // `missing_search` row) ahead of all others, then oldest `searched_at` first.
        // The acquisition sweep (RssSync/MissingItemSearch) takes only the first N per
        // run (bounded to protect the indexers) and stamps every node it searches via
        // `mark_missing_searched`, moving it to the BACK. So the budget rotates
        // deterministically through the whole backlog — every missing node is
        // guaranteed a search within ceil(backlog / N) runs — and a permanently
        // unsatisfiable node (every release rejected: below-min-seeders,
        // quality-not-allowed) can't monopolize the head: once searched it drops to
        // the back like everything else, costing exactly one slot per full cycle.
        // This replaces an earlier `ORDER BY RANDOM()` that avoided the same
        // starvation but gave no coverage guarantee, leaving grabbable-but-unlucky
        // items un-searched indefinitely. The trailing `RANDOM()` only breaks ties
        // within a stamp tier (notably the never-searched tier) so no stable sub-order
        // can form while a large tier drains. Mirrors `upgrade_candidates`. Portable
        // across SQLite and Postgres (`searched_at IS NULL` yields 1/0 / true-false;
        // DESC puts the never-searched first).
        let rows = sqlx::query(&pq(
            "SELECT c.id, c.library_id, c.media_type, c.parent_id, c.kind, c.series_type, c.coords,
                    c.monitored, c.title_id
             FROM content c
             LEFT JOIN missing_search m ON m.content_id = c.id
             WHERE c.monitored = 1
               AND c.kind IN ('movie', 'episode', 'track', 'book')
               AND NOT EXISTS (
                   SELECT 1 FROM content_file cf WHERE cf.content_id = c.id
               )
             ORDER BY (m.searched_at IS NULL) DESC, m.searched_at ASC, RANDOM()",
        ))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| row_to_node(r).map(|n| n.as_ref()))
            .collect()
    }

    async fn upsert(&self, node: &ContentNode) -> Result<()> {
        let id = node.id.to_string();
        let library_id = node.library_id.to_string();
        let media_type = serde_json::to_value(node.media_type)?
            .as_str()
            .unwrap_or_default()
            .to_string();
        let parent_id = node.parent_id.map(|p| p.to_string());
        let kind = kind_to_str(node.kind)?;
        let series_type = series_type_to_str(node.series_type);
        let coords = serde_json::to_string(&node.coords)?;
        let monitored = i64::from(node.monitored);
        let title_id = node.title_id.map(|t| t.to_string());

        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq(
                        "INSERT INTO content
                            (id, library_id, media_type, parent_id, kind, series_type, coords, monitored, title_id)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                         ON CONFLICT(id) DO UPDATE SET
                            library_id  = excluded.library_id,
                            media_type  = excluded.media_type,
                            parent_id   = excluded.parent_id,
                            kind        = excluded.kind,
                            series_type = excluded.series_type,
                            coords      = excluded.coords,
                            monitored   = excluded.monitored,
                            title_id    = excluded.title_id"),
                    )
                    .bind(id)
                    .bind(library_id)
                    .bind(media_type)
                    .bind(parent_id)
                    .bind(kind)
                    .bind(series_type)
                    .bind(coords)
                    .bind(monitored)
                    .bind(title_id)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    async fn children(&self, parent: ContentId) -> Result<Vec<ContentNode>> {
        // Ordered by id for a stable, deterministic walk; coords ordering would
        // require parsing the tagged JSON, which the adjacency-list walk does not
        // need. Callers that want numbering order sort on the decoded coords.
        let rows = sqlx::query(&pq(
            "SELECT id, library_id, media_type, parent_id, kind, series_type, coords, monitored, title_id
             FROM content WHERE parent_id = ?1 ORDER BY id ASC"),
        )
        .bind(parent.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_node).collect()
    }

    async fn roots(&self, library: LibraryId) -> Result<Vec<ContentNode>> {
        // Root nodes have no parent: a flat movie, or a series/artist/author the
        // tree hangs off of. Ordered by id for a stable list.
        let rows = sqlx::query(&pq(
            "SELECT id, library_id, media_type, parent_id, kind, series_type, coords, monitored, title_id
             FROM content WHERE library_id = ?1 AND parent_id IS NULL ORDER BY id ASC"),
        )
        .bind(library.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_node).collect()
    }

    async fn set_metadata(&self, id: ContentId, meta: &ContentMetadata) -> Result<()> {
        let id = id.to_string();
        let title = meta.title.clone();
        let year = meta.year.map(i64::from);
        let overview = meta.overview.clone();
        let runtime = meta.runtime.map(i64::from);
        let air_date = meta.air_date.clone();
        let digital_date = meta.digital_date.clone();
        // Genres are list-valued; store as a JSON array string so no association
        // table is needed. An empty list stores NULL (not "[]") to read back empty.
        let genres = if meta.genres.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&meta.genres).unwrap_or_default())
        };
        let rating = meta.rating.map(f64::from);
        let rating_votes = meta.rating_votes.map(i64::from);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq("INSERT INTO content_meta
                            (content_id, title, year, overview, runtime, air_date, digital_date,
                             genres, rating, rating_votes)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                         ON CONFLICT(content_id) DO UPDATE SET
                            title        = excluded.title,
                            year         = excluded.year,
                            overview     = excluded.overview,
                            runtime      = excluded.runtime,
                            air_date     = excluded.air_date,
                            digital_date = excluded.digital_date,
                            genres       = excluded.genres,
                            rating       = excluded.rating,
                            rating_votes = excluded.rating_votes"))
                    .bind(id)
                    .bind(title)
                    .bind(year)
                    .bind(overview)
                    .bind(runtime)
                    .bind(air_date)
                    .bind(digital_date)
                    .bind(genres)
                    .bind(rating)
                    .bind(rating_votes)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    async fn metadata(&self, id: ContentId) -> Result<Option<ContentMetadata>> {
        ContentRepo::metadata(self, id).await
    }

    async fn delete_movie(&self, id: ContentId) -> Result<Option<DeletedContent>> {
        self.delete_subtree(id, ContentKind::Movie).await
    }

    async fn delete_series(&self, id: ContentId) -> Result<Option<DeletedContent>> {
        self.delete_subtree(id, ContentKind::Series).await
    }
}

/// Decode a `content_meta` row into a [`ContentMetadata`]. The integer columns are
/// stored as SQLite `INTEGER` (i64) and narrowed back to the domain widths; a
/// value out of range is treated as absent rather than panicking (the metadata
/// source never emits a negative year/runtime, but the read path must stay
/// total).
fn row_to_content_metadata(row: crate::dialect::DbRow) -> Result<ContentMetadata> {
    let year: Option<i64> = row.try_get("year")?;
    let runtime: Option<i64> = row.try_get("runtime")?;
    let rating_votes: Option<i64> = row.try_get("rating_votes")?;
    let genres_json: Option<String> = row.try_get("genres")?;
    let genres = genres_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    let rating: Option<f64> = row.try_get("rating")?;
    Ok(ContentMetadata {
        title: row.try_get("title")?,
        year: year.and_then(|y| u16::try_from(y).ok()),
        overview: row.try_get("overview")?,
        runtime: runtime.and_then(|r| u32::try_from(r).ok()),
        air_date: row.try_get("air_date")?,
        digital_date: row.try_get("digital_date")?,
        genres,
        #[allow(clippy::cast_possible_truncation)]
        rating: rating.map(|r| r as f32),
        rating_votes: rating_votes.and_then(|v| u32::try_from(v).ok()),
    })
}
