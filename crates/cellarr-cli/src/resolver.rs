//! The live metadata resolver — the concrete [`MetadataResolver`] the
//! identify/refresh path resolves a node's rich metadata and artwork through.
//!
//! There was no concrete resolver before this: the trait existed and the pipeline
//! held an `Option<Arc<dyn DynMetadataResolver>>`, but nothing implemented it, so
//! `RefreshMetadata` was a no-op and added movies never gained overview, runtime,
//! genres, ratings, or posters. This fills that gap.
//!
//! Given a content node it:
//!   1. looks up the node's native external id (`tmdb` for movies, `tvdb` for TV);
//!   2. fetches the source's full details;
//!   3. maps them into the persisted [`ContentMetadata`] (overview / runtime /
//!      dates / genres / rating);
//!   4. caches the poster + fanart bytes under the MediaCover artwork dir.
//!
//! Degradation is non-negotiable: an unidentified node, or one whose source has no
//! API key, resolves to `Ok(None)` (nothing to persist) rather than an error.
//! Artwork fetches are best-effort — a failed image never fails the resolve.

use std::path::PathBuf;

use async_trait::async_trait;

use cellarr_core::repo::ContentRepository;
use cellarr_core::{
    ContentId, ContentKind, ContentMetadata, ContentNode, ContentRef, Coordinates, MediaType,
};
use cellarr_db::Database;
use cellarr_media::{ArtworkKind, MetadataResolver, ResolvedMetadata};
use cellarr_meta::{ChildNode, Fetcher, Image, Metadata, ReqwestFetcher, TheTvdbSource, TmdbSource};

use crate::config::Config;

/// How many times to attempt an artwork image download before treating it as a
/// miss. One initial try plus two retries turns most transient CDN blips (a TMDB
/// 5xx or connection reset under load) into a hit, so a freshly added film is not
/// left posterless until the next daily refresh.
const ARTWORK_FETCH_ATTEMPTS: u32 = 3;

/// The resolver's error: a genuine provider/repository failure the refresh caller
/// logs and moves past (a metadata refresh never blocks acquisition).
#[derive(Debug, thiserror::Error)]
pub enum ResolverError {
    /// A metadata source request failed (network/decoding), key present.
    #[error("metadata source error: {0}")]
    Source(String),
    /// A repository read failed while looking up the node's external id.
    #[error("repository error: {0}")]
    Repo(String),
}

/// The live `MetadataResolver`, holding the concrete TheTVDB (TV) and TMDb (movie)
/// sources plus the DB handle (to resolve a node's external id) and the artwork
/// fetcher + cache dir.
///
/// Generic over the [`Fetcher`] so the daemon binds the real `reqwest` transport
/// while tests bind a recorded one (the same mapping + artwork-caching logic runs
/// in both). `artwork_fetcher` is a separate fetcher used only for image bytes so
/// the recorded test path can supply artwork without a live host.
pub struct LiveMetadataResolver<F: Fetcher = ReqwestFetcher> {
    db: Database,
    tvdb: TheTvdbSource<F>,
    tmdb: TmdbSource<F>,
    tvdb_configured: bool,
    tmdb_configured: bool,
    artwork_fetcher: F,
    artwork_dir: PathBuf,
}

impl LiveMetadataResolver<ReqwestFetcher> {
    /// Build the live resolver from the loaded daemon config, the DB handle, and
    /// the MediaCover artwork dir. Keys come from `CELLARR_TVDB__*` / `CELLARR_TMDB__*`;
    /// an absent key leaves the corresponding source unavailable (degraded).
    #[must_use]
    pub fn from_config(config: &Config, db: Database, artwork_dir: PathBuf) -> Self {
        let tvdb_configured = config.tvdb.api_key.is_some();
        let tmdb_configured = config.tmdb.api_key.is_some();
        let tvdb = TheTvdbSource::new(
            ReqwestFetcher::new("thetvdb"),
            config.thetvdb_source_config(),
        );
        let tmdb = TmdbSource::new(ReqwestFetcher::new("tmdb"), config.tmdb_source_config());
        Self {
            db,
            tvdb,
            tmdb,
            tvdb_configured,
            tmdb_configured,
            artwork_fetcher: ReqwestFetcher::new("artwork"),
            artwork_dir,
        }
    }
}

impl<F: Fetcher> LiveMetadataResolver<F> {
    /// Construct directly from built sources (used by tests over a recorded
    /// fetcher). `*_configured` are the offline gates.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn from_sources(
        db: Database,
        tvdb: TheTvdbSource<F>,
        tmdb: TmdbSource<F>,
        tvdb_configured: bool,
        tmdb_configured: bool,
        artwork_fetcher: F,
        artwork_dir: PathBuf,
    ) -> Self {
        Self {
            db,
            tvdb,
            tmdb,
            tvdb_configured,
            tmdb_configured,
            artwork_fetcher,
            artwork_dir,
        }
    }

    /// Fetch a node's full details from the source matching its media type, or
    /// `None` when that source has no key (degraded — never an error).
    async fn fetch_details(
        &self,
        media_type: MediaType,
        external_id: &str,
    ) -> Result<Option<Metadata>, ResolverError> {
        match media_type {
            MediaType::Movie if self.tmdb_configured => self
                .tmdb
                .fetch_normalized(external_id)
                .await
                .map(Some)
                .map_err(|e| ResolverError::Source(e.to_string())),
            MediaType::Tv if self.tvdb_configured => self
                .tvdb
                .fetch_normalized(external_id)
                .await
                .map(Some)
                .map_err(|e| ResolverError::Source(e.to_string())),
            _ => Ok(None),
        }
    }

    /// Populate the grabbable Season/Episode tree under a series from the
    /// provider's episode list (`children`). This is what makes a TV series
    /// acquirable: the pipeline only searches/grabs leaf `Episode` nodes (the
    /// series container is excluded from `monitored_missing`), so without this a
    /// series never downloads.
    ///
    /// The tree mirrors the migration shape — `Series → Season → Episode` — and
    /// each Season/Episode node carries the **series title** as its identity
    /// (indexed for FTS + persisted metadata), because episode search resolves a
    /// node's own title (it does not walk to the series); the series title plus the
    /// season/episode coordinates are what the indexer query and file naming use
    /// (the episode name itself is not needed by either).
    ///
    /// Idempotent: existing seasons/episodes (matched by their coordinates) are
    /// left untouched, so a re-refresh only adds newly-aired episodes rather than
    /// duplicating the tree. Episodes inherit the series' `monitored` flag and
    /// `series_type` (the anime-numbering switch); specials (season 0) are created
    /// unmonitored so they populate the tree without auto-grabbing.
    async fn expand_series(
        &self,
        series: &ContentRef,
        series_title: &str,
        episodes: &[ChildNode],
    ) -> Result<(), ResolverError> {
        let repo = self.db.content();
        let series_node = repo
            .get_node(series.id)
            .await
            .map_err(|e| ResolverError::Repo(e.to_string()))?
            .ok_or_else(|| ResolverError::Repo(format!("series node {} not found", series.id)))?;

        // Existing tree: season number → season node id, and the set of
        // (season, episode) coordinates already present, so we never duplicate.
        let mut season_id: std::collections::HashMap<u32, ContentId> =
            std::collections::HashMap::new();
        let mut present: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        let seasons = repo
            .children(series.id)
            .await
            .map_err(|e| ResolverError::Repo(e.to_string()))?;
        for s in seasons {
            let Coordinates::Episode { season, .. } = s.coords else {
                continue;
            };
            if s.kind != ContentKind::Season {
                continue;
            }
            season_id.insert(season, s.id);
            for ep in repo
                .children(s.id)
                .await
                .map_err(|e| ResolverError::Repo(e.to_string()))?
            {
                if let Coordinates::Episode { season, episode, .. } = ep.coords {
                    present.insert((season, episode));
                }
            }
        }

        let mut created = 0usize;
        for ep in episodes {
            let (Some(season), Some(episode)) = (ep.season, ep.episode) else {
                continue; // an unnumbered provider entry is not a grabbable episode
            };
            if present.contains(&(season, episode)) {
                continue;
            }
            // Specials (season 0) are present-but-unmonitored; real seasons inherit
            // the series' monitored flag.
            let monitored = series_node.monitored && season != 0;

            // Ensure the season container exists.
            let parent_season = match season_id.get(&season) {
                Some(id) => *id,
                None => {
                    let id = ContentId::new();
                    let node = ContentNode {
                        id,
                        library_id: series.library_id,
                        media_type: MediaType::Tv,
                        parent_id: Some(series.id),
                        kind: ContentKind::Season,
                        series_type: series_node.series_type,
                        coords: Coordinates::Episode {
                            season,
                            episode: 0,
                            absolute: None,
                        },
                        monitored,
                        title_id: None,
                        tags: Vec::new(),
                    };
                    repo.upsert(&node)
                        .await
                        .map_err(|e| ResolverError::Repo(e.to_string()))?;
                    repo.index_title(id, series_title)
                        .await
                        .map_err(|e| ResolverError::Repo(e.to_string()))?;
                    season_id.insert(season, id);
                    id
                }
            };

            let id = ContentId::new();
            let node = ContentNode {
                id,
                library_id: series.library_id,
                media_type: MediaType::Tv,
                parent_id: Some(parent_season),
                kind: ContentKind::Episode,
                series_type: series_node.series_type,
                coords: Coordinates::Episode {
                    season,
                    episode,
                    absolute: ep.absolute,
                },
                monitored,
                title_id: None,
                tags: Vec::new(),
            };
            repo.upsert(&node)
                .await
                .map_err(|e| ResolverError::Repo(e.to_string()))?;
            // Identity = the SERIES title (search + naming both read the node's own
            // title; they never walk to the series), plus the air date for the
            // calendar. The episode name is intentionally not stored: neither
            // search nor the naming tokens use it.
            repo.index_title(id, series_title)
                .await
                .map_err(|e| ResolverError::Repo(e.to_string()))?;
            let meta = ContentMetadata {
                title: Some(series_title.to_string()),
                air_date: ep.air_date.clone(),
                ..Default::default()
            };
            repo.set_metadata(id, &meta)
                .await
                .map_err(|e| ResolverError::Repo(e.to_string()))?;
            present.insert((season, episode));
            created += 1;
        }
        if created > 0 {
            tracing::info!(series = %series.id, created, "series expansion: created episode nodes");
        }
        Ok(())
    }

    /// Cache the poster/fanart bytes under `<artwork_dir>/<content_id>/<kind>.jpg`
    /// and return which kinds were successfully cached. Best-effort: a failed
    /// download or write is skipped, never an error.
    ///
    /// The image fetch is **retried** ([`ARTWORK_FETCH_ATTEMPTS`]) before giving
    /// up: a single transient blip from the image CDN otherwise means a freshly
    /// added film silently has no poster until the next daily refresh — the
    /// "added a film, artwork never showed up" gap. A genuine miss (every attempt
    /// failed, or the provider offered no art) is logged at `warn`, not swallowed
    /// at `debug`, so a persistent problem is visible.
    async fn cache_artwork(&self, content_id: &str, images: &[Image]) -> Vec<ArtworkKind> {
        let mut kinds = Vec::new();
        let dir = self.artwork_dir.join(content_id);
        for image in images {
            let kind = match image.kind.as_str() {
                "poster" => ArtworkKind::Poster,
                "fanart" => ArtworkKind::Fanart,
                _ => continue,
            };
            let Some(bytes) = self.fetch_artwork_bytes(&image.url).await else {
                tracing::warn!(url = %image.url, kind = %kind.slug(), content = %content_id, "artwork fetch gave up after retries");
                continue;
            };
            if let Err(e) = tokio::fs::create_dir_all(&dir).await {
                tracing::debug!(dir = %dir.display(), error = %e, "artwork dir create failed");
                continue;
            }
            let path = dir.join(format!("{}.jpg", kind.slug()));
            match tokio::fs::write(&path, &bytes).await {
                Ok(()) => kinds.push(kind),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "artwork write failed")
                }
            }
        }
        kinds
    }

    /// Fetch one artwork image's bytes, retrying a transient failure (network
    /// error, non-2xx, empty body) up to [`ARTWORK_FETCH_ATTEMPTS`] with a short
    /// backoff. Returns `None` only when every attempt failed — the caller treats
    /// that as a (logged) miss. Image CDNs (TMDB) occasionally 5xx or reset under
    /// load; one retry turns most of those into a hit.
    async fn fetch_artwork_bytes(&self, url: &str) -> Option<Vec<u8>> {
        for attempt in 1..=ARTWORK_FETCH_ATTEMPTS {
            match self.artwork_fetcher.get(url, &[]).await {
                Ok(resp) if (200..300).contains(&resp.status) && !resp.body.is_empty() => {
                    return Some(resp.body);
                }
                Ok(resp) => {
                    tracing::debug!(url = %url, status = resp.status, attempt, "artwork fetch non-2xx/empty");
                }
                Err(e) => {
                    tracing::debug!(url = %url, error = %e, attempt, "artwork fetch failed");
                }
            }
            if attempt < ARTWORK_FETCH_ATTEMPTS {
                tokio::time::sleep(std::time::Duration::from_millis(500 * u64::from(attempt)))
                    .await;
            }
        }
        None
    }
}

/// Project a source's normalized [`Metadata`] into the persisted [`ContentMetadata`].
fn to_content_metadata(m: Metadata) -> ContentMetadata {
    ContentMetadata {
        title: Some(m.title),
        year: m.year,
        overview: m.overview,
        runtime: m.runtime,
        air_date: m.release_date,
        digital_date: m.digital_release,
        genres: m.genres,
        rating: m.rating,
        rating_votes: m.rating_votes,
    }
}

#[async_trait]
impl<F: Fetcher> MetadataResolver for LiveMetadataResolver<F> {
    type Error = ResolverError;

    #[tracing::instrument(
        name = "metadata.resolve",
        skip_all,
        fields(content_id = %content.id, media_type = ?content.media_type)
    )]
    async fn resolve(&self, content: &ContentRef) -> Result<Option<ResolvedMetadata>, Self::Error> {
        // The node's native external id (tmdb for movies, tvdb for TV). An
        // unidentified node has none — nothing to resolve.
        let external = self
            .db
            .content()
            .external_id_for(content.id, content.media_type)
            .await
            .map_err(|e| ResolverError::Repo(e.to_string()))?;
        let Some((scheme, external_id)) = external else {
            return Ok(None);
        };
        // Only the native scheme drives a details fetch — an imdb-only node cannot
        // be fetched by id from the tmdb/tvdb details endpoints.
        let native = matches!(
            (content.media_type, scheme.as_str()),
            (MediaType::Movie, "tmdb") | (MediaType::Tv, "tvdb")
        );
        if !native {
            return Ok(None);
        }
        let Some(meta) = self.fetch_details(content.media_type, &external_id).await? else {
            return Ok(None);
        };
        let images = meta.images.clone();
        // Series expansion: a TV series' fetched metadata carries its episode list
        // (`meta.children`). Populate the grabbable Season/Episode tree from it so
        // the acquisition pipeline has leaf episodes to search and grab — without
        // this a series is a lone container the pipeline can never act on. Only the
        // series node reaches here (episodes/seasons have no series_meta row, so
        // `external_id_for` returns None and they resolve to Ok(None) above), so
        // this fires once per series. Best-effort: an expansion failure is logged
        // and never fails the metadata resolve.
        if content.media_type == MediaType::Tv && !meta.children.is_empty() {
            if let Err(e) = self.expand_series(content, &meta.title, &meta.children).await {
                tracing::warn!(series = %content.id, error = %e, "series expansion failed");
            }
        }
        // Persist the series' alternate titles so the matcher can adopt a file named
        // by an alias (e.g. an English "Naruto" file onto "NARUTO－ナルト－").
        if content.media_type == MediaType::Tv && !meta.aliases.is_empty() {
            if let Err(e) = self
                .db
                .content()
                .set_series_aliases(content.id, &meta.aliases)
                .await
            {
                tracing::warn!(series = %content.id, error = %e, "persisting series aliases failed");
            }
        }
        let resolved_meta = to_content_metadata(meta);
        let artwork = self.cache_artwork(&content.id.to_string(), &images).await;
        Ok(Some(ResolvedMetadata {
            meta: resolved_meta,
            artwork,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::repo::{ContentRepository, ProfileRepository};
    use cellarr_core::{
        ContentId, ContentNode, Library, LibraryId, QualityProfile, QualityProfileId, SeriesType,
    };
    use cellarr_db::Database;
    use cellarr_meta::{HttpResponse, MetaError, TheTvdbConfig, TmdbConfig};

    /// A fetcher that never succeeds — series expansion touches only the DB (never
    /// the provider), so the sources are inert here.
    struct NoopFetcher;

    #[async_trait]
    impl cellarr_meta::Fetcher for NoopFetcher {
        async fn get(
            &self,
            _url: &str,
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, MetaError> {
            Ok(HttpResponse {
                status: 404,
                body: Vec::new(),
            })
        }
    }

    fn resolver(db: Database) -> LiveMetadataResolver<NoopFetcher> {
        LiveMetadataResolver::from_sources(
            db,
            TheTvdbSource::new(NoopFetcher, TheTvdbConfig::default()),
            TmdbSource::new(NoopFetcher, TmdbConfig::default()),
            true,
            true,
            NoopFetcher,
            std::env::temp_dir(),
        )
    }

    fn child(season: u32, episode: u32) -> ChildNode {
        ChildNode {
            source_id: None,
            season: Some(season),
            episode: Some(episode),
            absolute: Some(season * 100 + episode),
            air_date: Some("2008-01-20".into()),
            title: Some(format!("Episode {episode}")),
        }
    }

    async fn seed_series(db: &Database, monitored: bool) -> ContentRef {
        // A library (and its profile) must exist — content.library_id is a FK.
        let profile = QualityProfile {
            id: QualityProfileId::new(),
            name: "p".into(),
            allowed_qualities: vec![1],
            upgrades_allowed: false,
            cutoff_quality: 1,
            min_custom_format_score: 0,
            upgrade_until_custom_format_score: 0,
            required_languages: Vec::new(),
        };
        db.profiles().upsert_profile(&profile).await.unwrap();
        let library_id = LibraryId::new();
        let library = Library {
            id: library_id,
            media_type: MediaType::Tv,
            name: "TV".into(),
            root_folders: vec!["/unused".into()],
            default_quality_profile: profile.id,
        };
        db.config().upsert_library(&library).await.unwrap();
        let id = ContentId::new();
        let node = ContentNode {
            id,
            library_id,
            media_type: MediaType::Tv,
            parent_id: None,
            kind: ContentKind::Series,
            series_type: SeriesType::Standard,
            coords: Coordinates::Episode {
                season: 0,
                episode: 0,
                absolute: None,
            },
            monitored,
            title_id: None,
            tags: Vec::new(),
        };
        db.content().upsert(&node).await.unwrap();
        ContentRef::new(id, library_id, MediaType::Tv, node.coords).unwrap()
    }

    async fn open_db() -> (tempfile::TempDir, Database) {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
            .await
            .unwrap();
        (tmp, db)
    }

    #[tokio::test]
    async fn expand_series_creates_grabbable_monitored_episodes() {
        let (_tmp, db) = open_db().await;
        let series = seed_series(&db, true).await;
        let r = resolver(db.clone());

        // 3 real-season episodes + 1 special (season 0).
        let eps = vec![child(1, 1), child(1, 2), child(2, 1), child(0, 1)];
        r.expand_series(&series, "Breaking Bad", &eps).await.unwrap();

        // monitored_missing (what the pipeline grabs) now returns exactly the 3
        // real episodes — the series/season containers and the special are excluded.
        let missing = db.content().monitored_missing().await.unwrap();
        assert_eq!(
            missing.len(),
            3,
            "3 monitored episodes are grabbable; series/season/special excluded"
        );
        assert!(
            missing
                .iter()
                .all(|c| c.media_type == MediaType::Tv
                    && matches!(c.coords, Coordinates::Episode { episode, .. } if episode > 0)),
            "every grabbable node is a real TV episode"
        );

        // Each episode carries the SERIES title as its identity, so the TvModule's
        // search resolves "Breaking Bad" + the season/episode numbering.
        let ep = &missing[0];
        let meta = ContentRepository::metadata(&db.content(), ep.id)
            .await
            .unwrap()
            .expect("episode has metadata");
        assert_eq!(meta.title.as_deref(), Some("Breaking Bad"));
    }

    #[tokio::test]
    async fn expand_series_is_idempotent_and_adds_only_new_episodes() {
        let (_tmp, db) = open_db().await;
        let series = seed_series(&db, true).await;
        let r = resolver(db.clone());

        r.expand_series(&series, "Show", &[child(1, 1), child(1, 2)])
            .await
            .unwrap();
        assert_eq!(db.content().monitored_missing().await.unwrap().len(), 2);

        // Re-refresh with the same episodes plus a newly-aired one: no duplicates,
        // just the new episode appears.
        r.expand_series(&series, "Show", &[child(1, 1), child(1, 2), child(1, 3)])
            .await
            .unwrap();
        assert_eq!(
            db.content().monitored_missing().await.unwrap().len(),
            3,
            "a re-refresh adds only the new episode, never duplicates"
        );
    }

    #[tokio::test]
    async fn expand_series_of_unmonitored_series_grabs_nothing() {
        let (_tmp, db) = open_db().await;
        let series = seed_series(&db, false).await;
        let r = resolver(db.clone());
        r.expand_series(&series, "Show", &[child(1, 1), child(1, 2)])
            .await
            .unwrap();
        // Episodes are created (the tree exists) but inherit the series'
        // unmonitored flag, so none are acquisition targets.
        assert!(db.content().monitored_missing().await.unwrap().is_empty());
        assert_eq!(db.content().children(series.id).await.unwrap().len(), 1, "one season node created");
    }
}
