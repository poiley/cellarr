//! Serializable configuration rows.
//!
//! These are the persisted, user-managed configuration aggregates that the rest
//! of the system reads to know *what* is configured (root folders, indexers,
//! download clients, notifications). Each carries the small set of fields the
//! pipeline reasons about generically, plus a `settings: serde_json::Value` for
//! the adapter-specific bits core deliberately stays ignorant of (an indexer's
//! API key and URL, a download client's host/port/category mapping, a
//! notification target's webhook URL, …). The adapter crate that owns a kind
//! deserializes `settings` into its own typed struct.
//!
//! Keeping the common fields typed and the long tail in one validated JSON column
//! follows the data-model decision in [`docs/02-data-model.md`]: typed where the
//! shape is shared, JSON only for the genuinely open-ended remainder.

use serde::{Deserialize, Serialize};

use crate::ids::{DownloadClientId, IndexerId};
use crate::release::Protocol;
use crate::{MediaType, SeriesType};

/// Media-management settings: the cross-library file-handling policy.
///
/// This is the small set of file-handling toggles the *arr ecosystem groups
/// under "Media Management". cellarr keeps only the fields its file operations
/// actually reason about; the long tail of cosmetic naming options stays in the
/// `/api/v3` projection, not here.
///
/// The headline field is [`recycle_bin_path`](Self::recycle_bin_path): when set,
/// a content delete that removes media **moves** the files into the recycle bin
/// (preserving their layout relative to the library root) instead of unlinking
/// them, so a mistaken delete is reversible. `None` (the default) unlinks
/// directly, matching the *arr default of an empty recycle-bin path. Mirrors
/// Sonarr/Radarr `recycleBin`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaManagement {
    /// The recycle-bin directory deleted media is moved into instead of being
    /// unlinked. `None`/empty means delete unlinks the file outright (the *arr
    /// default). An absolute path: deleted files land under it, preserving their
    /// path relative to the library root so a restore is unambiguous.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recycle_bin_path: Option<String>,
    /// The per-media-type on-disk naming formats the rename engine renders against
    /// each module's [`NamingTokens`](crate::NamingTokens). Defaults reproduce the
    /// daemon's built-in *arr-conventional layout, so a zero-config library keeps
    /// renaming exactly as before; a user customizing these changes the rendered
    /// paths without touching code.
    #[serde(default)]
    pub naming: NamingFormats,
    /// The Unix file/folder permission policy applied to imported media *after* the
    /// crash-safe commit. Best-effort and Unix-only: a failure to chmod/chown is
    /// logged and never rolls back or corrupts the imported media.
    #[serde(default)]
    pub permissions: ImportPermissions,
    /// Whether (and which) sibling "extra" files (subtitles, `.nfo`, …) are
    /// imported alongside a media file, renamed to match it.
    #[serde(default)]
    pub extra_files: ExtraFileImport,
    /// Whether the Kodi/Jellyfin `.nfo` metadata sidecar is written next to
    /// imported media (a `movie.nfo` / `tvshow.nfo` / per-episode `.nfo`). This is
    /// the enable flag the v3 `metadata` consumer resource toggles. Defaults to
    /// `true` so an existing library keeps writing sidecars exactly as before this
    /// became configurable; a user can turn the consumer off to suppress them.
    #[serde(default = "default_true")]
    pub write_nfo: bool,
}

impl Default for MediaManagement {
    /// The zero-config defaults. Note `write_nfo` defaults to `true` (matching the
    /// serde default), so a library with no persisted settings still writes the
    /// `.nfo` sidecars it always did — a derived `Default` would wrongly disable
    /// them.
    fn default() -> Self {
        Self {
            recycle_bin_path: None,
            naming: NamingFormats::default(),
            permissions: ImportPermissions::default(),
            extra_files: ExtraFileImport::default(),
            write_nfo: true,
        }
    }
}

/// The user-configurable on-disk naming formats, one per nameable surface.
///
/// A library lays media out as `<series folder>/<season folder>/<episode file>`
/// for TV and `<movie folder>/<movie file>` for movies. Each piece is its own
/// template so the UI can present them independently (matching the *arr "Media
/// Management → Naming" tabs), and the rename engine composes the full relative
/// path from them via [`episode_format`](Self::episode_format) /
/// [`movie_format`](Self::movie_format).
///
/// Every field carries the daemon's prior built-in default, so an absent or
/// partial config renders identically to the hardcoded behavior it replaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamingFormats {
    /// The TV series *folder* name, e.g. `{Series Title}`.
    #[serde(default = "default_series_folder_format")]
    pub series_folder_format: String,
    /// The TV *season* folder name nested under the series folder, e.g.
    /// `Season {Season}`.
    #[serde(default = "default_season_folder_format")]
    pub season_folder_format: String,
    /// The TV *episode* file name (extension included), e.g.
    /// `{Series Title} - S{Season}E{Episode}.{Extension}`.
    #[serde(default = "default_episode_file_format")]
    pub episode_file_format: String,
    /// The TV *anime* episode file name (extension included), rendered for an
    /// episode whose series root is [`SeriesType::Anime`] **and** whose absolute
    /// number is known, e.g. `{Series Title} - {Absolute Episode} - S{Season}E{Episode}.{Extension}`.
    /// Uses the [`{Absolute Episode}`] token the anime numbering supplies. When an
    /// anime episode has no known absolute number the rename engine falls back to
    /// [`episode_file_format`](Self::episode_file_format) so it never renders a
    /// broken name (a dangling ` -  - `); see
    /// [`episode_format_for`](Self::episode_format_for).
    #[serde(default = "default_anime_episode_file_format")]
    pub anime_episode_file_format: String,
    /// The movie *file* name (extension included), rendered inside the movie
    /// folder, e.g. `{Movie Title} ({Release Year})/{Movie Title}.{Extension}`.
    #[serde(default = "default_movie_file_format")]
    pub movie_file_format: String,
}

fn default_series_folder_format() -> String {
    "{Series Title}".to_string()
}
fn default_season_folder_format() -> String {
    "Season {Season}".to_string()
}
fn default_episode_file_format() -> String {
    "{Series Title} - S{Season}E{Episode}.{Extension}".to_string()
}
fn default_anime_episode_file_format() -> String {
    "{Series Title} - {Absolute Episode} - S{Season}E{Episode}.{Extension}".to_string()
}
fn default_movie_file_format() -> String {
    "{Movie Title} ({Release Year})/{Movie Title}.{Extension}".to_string()
}

impl Default for NamingFormats {
    fn default() -> Self {
        Self {
            series_folder_format: default_series_folder_format(),
            season_folder_format: default_season_folder_format(),
            episode_file_format: default_episode_file_format(),
            anime_episode_file_format: default_anime_episode_file_format(),
            movie_file_format: default_movie_file_format(),
        }
    }
}

/// A nameable on-disk surface a [`NamingFormats`] template configures. Used to
/// select which template to render and to advertise the matching token vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NameTarget {
    /// The movie file (and its enclosing folder).
    MovieFile,
    /// The TV series folder.
    SeriesFolder,
    /// The TV season folder.
    SeasonFolder,
    /// The TV episode file.
    EpisodeFile,
}

impl NameTarget {
    /// Every name target, in UI display order.
    #[must_use]
    pub fn all() -> [NameTarget; 4] {
        [
            NameTarget::MovieFile,
            NameTarget::SeriesFolder,
            NameTarget::SeasonFolder,
            NameTarget::EpisodeFile,
        ]
    }

    /// A stable lowercase key for this target (used in API payloads).
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            NameTarget::MovieFile => "movieFile",
            NameTarget::SeriesFolder => "seriesFolder",
            NameTarget::SeasonFolder => "seasonFolder",
            NameTarget::EpisodeFile => "episodeFile",
        }
    }

    /// The media type this target belongs to.
    #[must_use]
    pub fn media_type(self) -> MediaType {
        match self {
            NameTarget::MovieFile => MediaType::Movie,
            NameTarget::SeriesFolder | NameTarget::SeasonFolder | NameTarget::EpisodeFile => {
                MediaType::Tv
            }
        }
    }
}

impl NamingFormats {
    /// The configured template for a single [`NameTarget`].
    #[must_use]
    pub fn template(&self, target: NameTarget) -> &str {
        match target {
            NameTarget::MovieFile => &self.movie_file_format,
            NameTarget::SeriesFolder => &self.series_folder_format,
            NameTarget::SeasonFolder => &self.season_folder_format,
            NameTarget::EpisodeFile => &self.episode_file_format,
        }
    }

    /// The composed full relative path format for a TV episode:
    /// `<series folder>/<season folder>/<episode file>`. Empty pieces are
    /// dropped so a flat layout (no season folder) renders without a `//`.
    #[must_use]
    pub fn episode_format(&self) -> String {
        [
            self.series_folder_format.trim_matches('/'),
            self.season_folder_format.trim_matches('/'),
            self.episode_file_format.trim_matches('/'),
        ]
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/")
    }

    /// The composed full relative path format for an **anime** TV episode:
    /// `<series folder>/<season folder>/<anime episode file>`. Composed exactly like
    /// [`episode_format`](Self::episode_format) but substituting the
    /// [`anime_episode_file_format`](Self::anime_episode_file_format) for the episode
    /// file segment, so the series/season folder layout is shared and only the file
    /// name differs (it carries the `{Absolute Episode}` token).
    #[must_use]
    pub fn anime_episode_format(&self) -> String {
        [
            self.series_folder_format.trim_matches('/'),
            self.season_folder_format.trim_matches('/'),
            self.anime_episode_file_format.trim_matches('/'),
        ]
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/")
    }

    /// The composed full relative path format for a movie. The movie file format
    /// already carries its enclosing folder (`{Movie Title} (…)/{Movie Title}.…`),
    /// so it is returned as-is.
    #[must_use]
    pub fn movie_format(&self) -> String {
        self.movie_file_format.clone()
    }

    /// The full relative-path naming format to render for a given media type —
    /// the single string the rename engine consumes.
    #[must_use]
    pub fn format_for(&self, media_type: MediaType) -> String {
        match media_type {
            MediaType::Movie => self.movie_format(),
            MediaType::Tv => self.episode_format(),
            // Music/books are not v1 naming surfaces; fall back to a bare title.
            MediaType::Music | MediaType::Book => "{Title}.{Extension}".to_string(),
        }
    }

    /// The TV episode format to render for an episode, choosing the **anime**
    /// episode format only when the series is [`SeriesType::Anime`] *and* the
    /// episode's absolute number is known.
    ///
    /// This is the graceful-fallback rule the rename engine relies on: the anime
    /// format references the `{Absolute Episode}` token, which the numbering only
    /// supplies once Identify has reconciled the absolute number. An anime episode
    /// whose absolute is still unknown (`has_absolute == false`) therefore falls
    /// back to [`episode_format`](Self::episode_format) so it never renders a broken
    /// name (a dangling ` -  - ` where the absolute would have gone). A non-anime
    /// series always uses the standard episode format regardless of `has_absolute`.
    #[must_use]
    pub fn episode_format_for(&self, series_type: SeriesType, has_absolute: bool) -> String {
        if series_type == SeriesType::Anime && has_absolute {
            self.anime_episode_format()
        } else {
            self.episode_format()
        }
    }

    /// The full relative-path naming format to render for a node, accounting for a
    /// TV series' [`SeriesType`] (so an anime episode with a known absolute number
    /// renders the anime format). For non-TV media types this is identical to
    /// [`format_for`](Self::format_for); for TV it delegates to
    /// [`episode_format_for`](Self::episode_format_for).
    #[must_use]
    pub fn format_for_series(
        &self,
        media_type: MediaType,
        series_type: SeriesType,
        has_absolute: bool,
    ) -> String {
        match media_type {
            MediaType::Tv => self.episode_format_for(series_type, has_absolute),
            other => self.format_for(other),
        }
    }
}

/// The Unix permission policy applied to imported media after commit.
///
/// All fields are optional; an empty policy (the default) applies nothing and the
/// imported file keeps the mode/ownership the copy produced. Mirrors the *arr
/// "chmod Folder", "chmod File", and "chown" media-management settings. **Unix
/// only and strictly best-effort** — applied *after* the media is durably
/// committed, so any chmod/chown failure is logged and never corrupts or rolls
/// back the import.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPermissions {
    /// The octal mode applied to created folders, as a string (e.g. `"755"`).
    /// `None`/empty leaves folder modes untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chmod_folder: Option<String>,
    /// The octal mode applied to imported files, as a string (e.g. `"644"`).
    /// `None`/empty leaves file modes untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chmod_file: Option<String>,
    /// The owner to chown imported media to: `"user"`, `":group"`, or
    /// `"user:group"`. Either side may be a numeric id or a name. `None`/empty
    /// leaves ownership untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chown: Option<String>,
}

/// Whether and which sibling extra files are imported alongside a media file.
///
/// When importing `Show.S01E01.mkv`, sibling files sharing its basename and
/// carrying one of [`extensions`](Self::extensions) (e.g. `Show.S01E01.en.srt`)
/// are imported next to the renamed media with the media's new basename
/// (`… - S01E01.en.srt`). Best-effort: an extra-file failure never breaks the
/// media import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraFileImport {
    /// Whether sibling extra files are imported at all. Default `true`: a release's
    /// subtitles (and `.nfo`) should follow the movie into the library by default —
    /// leaving them behind in the download is a silent loss once it is cleaned up.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The lowercase extensions (without the dot) treated as importable extras.
    #[serde(default = "default_extra_extensions")]
    pub extensions: Vec<String>,
}

fn default_extra_extensions() -> Vec<String> {
    ["srt", "sub", "idx", "ass", "ssa", "vtt", "nfo"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

impl Default for ExtraFileImport {
    fn default() -> Self {
        Self {
            enabled: true,
            extensions: default_extra_extensions(),
        }
    }
}

impl ExtraFileImport {
    /// Whether a given extension (with or without a leading dot, any case) is a
    /// configured importable extra.
    #[must_use]
    pub fn matches_extension(&self, ext: &str) -> bool {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        self.extensions
            .iter()
            .any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext))
    }
}

/// A configured root folder a library imports into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootFolder {
    /// Folder identifier.
    pub id: String,
    /// Absolute path on disk.
    pub path: String,
    /// Human-facing label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether the folder is currently enabled for imports.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// A configured indexer (Torznab, Newznab, Cardigann).
///
/// `Eq` is intentionally not derived: [`IndexerCriteria`] carries floating-point
/// seed targets, so equality is `PartialEq` only (sufficient for `assert_eq!` and
/// round-trip checks; the config is never used as a set/map key).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexerConfig {
    /// Indexer identifier.
    pub id: IndexerId,
    /// Human-facing name.
    pub name: String,
    /// The adapter kind (e.g. "torznab", "newznab"), selecting which
    /// implementation deserializes `settings`.
    pub kind: String,
    /// The download protocol this indexer's releases use.
    pub protocol: Protocol,
    /// Whether the indexer is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Priority for ordering/tie-breaking (lower is preferred, matching the
    /// *arr convention). When two otherwise-equal releases are found, the one from
    /// the lower-priority-number indexer wins (see the decision engine).
    #[serde(default)]
    pub priority: i32,
    /// The torrent release-acceptance criteria this indexer imposes, mirroring the
    /// Sonarr/Radarr per-indexer `minimumSeeders` + `seedCriteria` + flag-required
    /// settings. Default (all-`None`/empty) gates nothing. Usenet indexers ignore
    /// these (seeders/seed-time/freeleech are torrent concepts).
    #[serde(default)]
    pub criteria: IndexerCriteria,
    /// The tag ids this indexer is **scoped** to. A tagged indexer is searched
    /// only for content sharing at least one of these tags; an empty list (the
    /// default) is global — the indexer applies to all content, preserving prior
    /// behavior. Mirrors Sonarr/Radarr per-indexer tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<u32>,
    /// Adapter-specific settings (base URL, API key, categories, …).
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// Per-indexer release-acceptance criteria and seed policy.
///
/// These mirror the torrent fields the *arr indexer schema exposes
/// (`minimumSeeders`, `seedCriteria.seedRatio`, `seedCriteria.seedTime`) plus a
/// freeleech/required-flag gate. The decision engine honours them: a release below
/// the seeder floor or missing a required flag is rejected, and the seed
/// ratio/time become the [`RemovePolicy`](crate) the tracker uses to ratio/time-gate
/// the torrent's removal once it has been grabbed from this indexer.
///
/// Every field is optional so an indexer with no criteria configured gates
/// nothing — preserving the prior behaviour of accepting any matched release.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerCriteria {
    /// The minimum number of seeders a torrent release must advertise to be
    /// grabbed. A release below this (or with no reported seeders when this is set)
    /// is rejected. `None` accepts any seeder count. Mirrors `minimumSeeders`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_seeders: Option<u32>,
    /// The seed ratio target applied to a torrent grabbed from this indexer: the
    /// tracker keeps the torrent seeding until this ratio is met (or the seed time
    /// below, whichever comes first). `None` leaves removal to the global policy.
    /// Mirrors `seedCriteria.seedRatio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed_ratio: Option<f64>,
    /// The seed-time target **in minutes** applied to a torrent grabbed from this
    /// indexer (the tracker keeps it seeding until this elapses, or the ratio
    /// above). `None` leaves removal to the global policy. Mirrors
    /// `seedCriteria.seedTime`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed_time_minutes: Option<u64>,
    /// Indexer flags a release **must** carry to be grabbed, normalized to
    /// lowercase (e.g. `["freeleech"]`). A release missing any required flag is
    /// rejected. Empty (the default) requires nothing — the common "freeleech only"
    /// policy is `["freeleech"]`. Matched case-insensitively against the release's
    /// [`indexer_flags`](crate::Release::indexer_flags).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_flags: Vec<String>,
}

impl IndexerCriteria {
    /// Whether a torrent `release` satisfies the seeder floor.
    ///
    /// With no floor configured every release passes. With a floor set, a release
    /// that reports fewer seeders — or reports none at all — fails (an unreported
    /// seeder count cannot be proven to meet a floor).
    #[must_use]
    pub fn meets_seeder_floor(&self, seeders: Option<u32>) -> bool {
        match self.minimum_seeders {
            None => true,
            Some(floor) => seeders.is_some_and(|s| s >= floor),
        }
    }

    /// Whether `flags` carries every required flag (case-insensitively). With no
    /// required flags every release passes; the freeleech-only policy is a single
    /// required `"freeleech"` flag.
    #[must_use]
    pub fn has_required_flags(&self, flags: &[String]) -> bool {
        self.required_flags.iter().all(|required| {
            flags
                .iter()
                .any(|present| present.eq_ignore_ascii_case(required))
        })
    }

    /// The seed ratio/time targets expressed as `(min_ratio, min_seeding_time_secs)`
    /// for the download client's removal policy, converting the configured minutes
    /// to seconds. Either may be `None`.
    #[must_use]
    pub fn seed_targets_secs(&self) -> (Option<f64>, Option<u64>) {
        (
            self.seed_ratio,
            self.seed_time_minutes.map(|m| m.saturating_mul(60)),
        )
    }
}

/// A configured download client (qBittorrent, Deluge, Transmission, SABnzbd,
/// NZBGet).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadClientConfig {
    /// Client identifier.
    pub id: DownloadClientId,
    /// Human-facing name.
    pub name: String,
    /// The adapter kind (e.g. "qbittorrent", "sabnzbd"), selecting which
    /// implementation deserializes `settings`.
    pub kind: String,
    /// The download protocol this client handles.
    pub protocol: Protocol,
    /// Whether the client is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Priority for ordering/tie-breaking (lower is preferred).
    #[serde(default)]
    pub priority: i32,
    /// The category/label cellarr tags its downloads with so it only ever
    /// touches its own downloads.
    pub category: String,
    /// The tag ids this download client is **scoped** to. A tagged client is
    /// chosen only for content sharing at least one of these tags; an empty list
    /// (the default) is global — the client applies to all content, preserving
    /// prior behavior. Mirrors Sonarr/Radarr per-client tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<u32>,
    /// Adapter-specific settings (host, port, credentials, paths, …).
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// A configured notification target (Discord, webhook, email, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationConfig {
    /// Notification identifier.
    pub id: String,
    /// Human-facing name.
    pub name: String,
    /// The adapter kind (e.g. "discord", "webhook"), selecting which
    /// implementation deserializes `settings`.
    pub kind: String,
    /// Whether the notification is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The lifecycle events this target fires on, as stable string keys
    /// (e.g. "grab", "import", "upgrade", "health"). Empty means "all".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_events: Vec<String>,
    /// The tag ids this notification is **scoped** to. A tagged notification
    /// fires only for content sharing at least one of these tags; an empty list
    /// (the default) is global — it fires for all content, preserving prior
    /// behavior. Mirrors Sonarr/Radarr per-notification tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<u32>,
    /// Adapter-specific settings (webhook URL, channel, credentials, …).
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// A remote-path mapping: how to translate a download client's reported path
/// into a path cellarr can see.
///
/// When the download client and cellarr run on different hosts (or in different
/// containers with different mounts), the client reports a finished download at a
/// path that does not exist from cellarr's vantage point — e.g. the client says
/// `/downloads/Show.S01E01` but cellarr sees that same content at
/// `/data/downloads/Show.S01E01`. A mapping rewrites the client-reported
/// `content_path` from its [`remote_path`](Self::remote_path) prefix to the
/// [`local_path`](Self::local_path) prefix **before Import**.
///
/// This is a *shared* layer applied in one place (the jobs runner), not per
/// adapter: every download client benefits, and the blackhole adapter — which is
/// itself just a folder pair — composes with it cleanly. It mirrors the
/// Sonarr/Radarr `RemotePathMapping` the ecosystem (Recyclarr, UoMi) expects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePathMapping {
    /// Mapping identifier.
    pub id: String,
    /// The download client host the mapping applies to (matched against the
    /// client's configured host). The Sonarr/Radarr convention; cellarr matches
    /// it case-insensitively and treats an empty host as "any host".
    #[serde(default)]
    pub host: String,
    /// The path prefix as the **download client** reports it (e.g.
    /// `/downloads/`).
    pub remote_path: String,
    /// The path prefix as **cellarr** sees the same location (e.g.
    /// `/data/downloads/`).
    pub local_path: String,
}

impl RemotePathMapping {
    /// Apply this mapping to a client-reported `path`, returning the rewritten
    /// path when `path` starts with [`remote_path`](Self::remote_path), or `None`
    /// when it does not match (so the caller can try the next mapping or pass the
    /// path through unchanged).
    ///
    /// Matching is a prefix replacement that respects path boundaries: a
    /// `remote_path` of `/downloads` matches `/downloads/x` and `/downloads`
    /// itself, but not `/downloads-extra/x`. Trailing slashes on either prefix are
    /// normalized so `/downloads` and `/downloads/` behave identically.
    #[must_use]
    pub fn rewrite(&self, path: &str) -> Option<String> {
        let remote = self.remote_path.trim_end_matches('/');
        let local = self.local_path.trim_end_matches('/');
        if remote.is_empty() {
            return None;
        }
        if path == remote {
            return Some(local.to_string());
        }
        let rest = path.strip_prefix(remote)?;
        // Only a boundary match counts: the char after the prefix must be a
        // separator, otherwise `/downloads` would wrongly match `/downloads-x`.
        if rest.starts_with('/') {
            Some(format!("{local}{rest}"))
        } else {
            None
        }
    }

    /// Whether this mapping applies to a download client on `client_host`.
    /// An empty mapping host matches any client; otherwise the comparison is
    /// case-insensitive.
    #[must_use]
    pub fn matches_host(&self, client_host: &str) -> bool {
        self.host.is_empty() || self.host.eq_ignore_ascii_case(client_host)
    }
}

/// Apply the first matching [`RemotePathMapping`] in `mappings` to a
/// client-reported `content_path`, returning the rewritten path (or the original
/// unchanged when none match).
///
/// This is the single shared entry point the pipeline calls before Import so the
/// rewrite lives in exactly one place regardless of which download client
/// produced the path. Mappings are tried in order; the first whose
/// [`host`](RemotePathMapping::host) and [`remote_path`](RemotePathMapping::remote_path)
/// match wins. An empty mapping list (the default) is a no-op.
#[must_use]
pub fn apply_remote_path_mappings(
    mappings: &[RemotePathMapping],
    client_host: &str,
    content_path: &str,
) -> String {
    for mapping in mappings {
        if mapping.matches_host(client_host) {
            if let Some(rewritten) = mapping.rewrite(content_path) {
                return rewritten;
            }
        }
    }
    content_path.to_string()
}

/// The serde default for an `enabled` flag: configuration is enabled unless
/// explicitly turned off.
const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(remote: &str, local: &str) -> RemotePathMapping {
        RemotePathMapping {
            id: "m1".into(),
            host: String::new(),
            remote_path: remote.into(),
            local_path: local.into(),
        }
    }

    #[test]
    fn rewrite_replaces_matching_prefix() {
        let m = mapping("/downloads", "/data/downloads");
        assert_eq!(
            m.rewrite("/downloads/Show.S01E01"),
            Some("/data/downloads/Show.S01E01".into())
        );
    }

    #[test]
    fn rewrite_normalizes_trailing_slash() {
        let m = mapping("/downloads/", "/data/downloads/");
        assert_eq!(m.rewrite("/downloads/x"), Some("/data/downloads/x".into()));
    }

    #[test]
    fn rewrite_respects_path_boundary() {
        let m = mapping("/downloads", "/data/downloads");
        assert_eq!(m.rewrite("/downloads-extra/x"), None);
    }

    #[test]
    fn rewrite_matches_exact_prefix() {
        let m = mapping("/downloads", "/data/downloads");
        assert_eq!(m.rewrite("/downloads"), Some("/data/downloads".into()));
    }

    #[test]
    fn apply_passes_through_unmapped() {
        let maps = [mapping("/downloads", "/data/downloads")];
        assert_eq!(
            apply_remote_path_mappings(&maps, "", "/media/other/file"),
            "/media/other/file"
        );
    }

    #[test]
    fn apply_uses_first_matching() {
        let maps = [
            mapping("/a", "/local/a"),
            mapping("/downloads", "/data/downloads"),
        ];
        assert_eq!(
            apply_remote_path_mappings(&maps, "", "/downloads/x"),
            "/data/downloads/x"
        );
    }

    #[test]
    fn host_scopes_mapping() {
        let mut m = mapping("/downloads", "/data/downloads");
        m.host = "qbit.local".into();
        assert!(m.matches_host("qbit.local"));
        assert!(m.matches_host("QBIT.LOCAL"));
        assert!(!m.matches_host("other.host"));
    }

    #[test]
    fn naming_defaults_compose_arr_conventional_layout() {
        let n = NamingFormats::default();
        assert_eq!(
            n.format_for(MediaType::Tv),
            "{Series Title}/Season {Season}/{Series Title} - S{Season}E{Episode}.{Extension}"
        );
        assert_eq!(
            n.format_for(MediaType::Movie),
            "{Movie Title} ({Release Year})/{Movie Title}.{Extension}"
        );
    }

    #[test]
    fn naming_anime_format_composes_with_absolute_token() {
        let n = NamingFormats::default();
        assert_eq!(
            n.anime_episode_format(),
            "{Series Title}/Season {Season}/{Series Title} - {Absolute Episode} - S{Season}E{Episode}.{Extension}"
        );
    }

    #[test]
    fn episode_format_for_selects_anime_only_with_absolute() {
        let n = NamingFormats::default();
        // Anime + known absolute -> the anime file format.
        assert_eq!(
            n.episode_format_for(SeriesType::Anime, true),
            n.anime_episode_format()
        );
        // Anime but no known absolute -> graceful fall back to the standard format.
        assert_eq!(
            n.episode_format_for(SeriesType::Anime, false),
            n.episode_format()
        );
        // A standard/daily series never uses the anime format, even with an absolute.
        assert_eq!(
            n.episode_format_for(SeriesType::Standard, true),
            n.episode_format()
        );
        assert_eq!(
            n.episode_format_for(SeriesType::Daily, true),
            n.episode_format()
        );
    }

    #[test]
    fn format_for_series_routes_by_media_type() {
        let n = NamingFormats::default();
        // TV anime with absolute -> anime format; movie ignores series type.
        assert_eq!(
            n.format_for_series(MediaType::Tv, SeriesType::Anime, true),
            n.anime_episode_format()
        );
        assert_eq!(
            n.format_for_series(MediaType::Movie, SeriesType::Anime, true),
            n.movie_format()
        );
    }

    #[test]
    fn naming_episode_format_drops_empty_season_folder() {
        let n = NamingFormats {
            season_folder_format: String::new(),
            ..Default::default()
        };
        assert_eq!(
            n.episode_format(),
            "{Series Title}/{Series Title} - S{Season}E{Episode}.{Extension}"
        );
    }

    #[test]
    fn naming_custom_template_is_honored_per_target() {
        let n = NamingFormats {
            season_folder_format: "S{Season}".to_string(),
            ..Default::default()
        };
        assert_eq!(n.template(NameTarget::SeasonFolder), "S{Season}");
        assert_eq!(
            n.format_for(MediaType::Tv),
            "{Series Title}/S{Season}/{Series Title} - S{Season}E{Episode}.{Extension}"
        );
    }

    #[test]
    fn media_management_partial_json_keeps_naming_defaults() {
        // A config that only sets the recycle bin must deserialize with the full
        // default naming/permissions/extra-files, so existing rows upgrade cleanly.
        let mm: MediaManagement = serde_json::from_str(r#"{"recycleBinPath":"/recycle"}"#).unwrap();
        assert_eq!(mm.recycle_bin_path.as_deref(), Some("/recycle"));
        assert_eq!(mm.naming, NamingFormats::default());
        // Extra-file import is on by default, so subtitles follow the movie.
        assert!(mm.extra_files.enabled);
        assert!(mm.permissions.chmod_file.is_none());
    }

    #[test]
    fn extra_files_enabled_defaults_true_when_omitted() {
        // An `extraFiles` block that customizes only the extension list still leaves
        // importing ON — the field-level default is `true`, not the bool default.
        let x: ExtraFileImport = serde_json::from_str(r#"{"extensions":["srt"]}"#).unwrap();
        assert!(x.enabled);
        assert_eq!(x.extensions, vec!["srt".to_string()]);
    }

    #[test]
    fn naming_anime_episode_format_round_trips_through_json() {
        // The dedicated anime episode format survives a serialize → deserialize.
        let n = NamingFormats {
            anime_episode_file_format:
                "{Series Title} - {Absolute Episode} - {Episode}.{Extension}".to_string(),
            ..NamingFormats::default()
        };
        let json = serde_json::to_string(&n).unwrap();
        assert!(json.contains("animeEpisodeFileFormat"));
        let back: NamingFormats = serde_json::from_str(&json).unwrap();
        assert_eq!(n, back);
        assert_eq!(
            back.anime_episode_file_format,
            "{Series Title} - {Absolute Episode} - {Episode}.{Extension}"
        );
    }

    #[test]
    fn naming_partial_json_keeps_anime_default() {
        // A pre-anime-field naming row (no animeEpisodeFileFormat) decodes with the
        // built-in anime default, so existing rows upgrade cleanly.
        let n: NamingFormats =
            serde_json::from_str(r#"{"seriesFolderFormat":"{Series Title}"}"#).unwrap();
        assert_eq!(
            n.anime_episode_file_format,
            default_anime_episode_file_format()
        );
    }

    #[test]
    fn media_management_round_trips_through_json() {
        let mut mm = MediaManagement::default();
        mm.naming.movie_file_format = "{Movie Title}/movie.{Extension}".to_string();
        mm.permissions.chmod_file = Some("644".to_string());
        mm.permissions.chown = Some("media:media".to_string());
        mm.extra_files.enabled = true;
        let json = serde_json::to_string(&mm).unwrap();
        let back: MediaManagement = serde_json::from_str(&json).unwrap();
        assert_eq!(mm, back);
    }

    #[test]
    fn extra_file_extension_match_is_case_and_dot_insensitive() {
        let x = ExtraFileImport::default();
        assert!(x.matches_extension("srt"));
        assert!(x.matches_extension(".SRT"));
        assert!(x.matches_extension("Nfo"));
        assert!(!x.matches_extension("mkv"));
    }

    #[test]
    fn name_target_media_type_mapping() {
        assert_eq!(NameTarget::MovieFile.media_type(), MediaType::Movie);
        assert_eq!(NameTarget::SeriesFolder.media_type(), MediaType::Tv);
        assert_eq!(NameTarget::EpisodeFile.media_type(), MediaType::Tv);
    }
}
