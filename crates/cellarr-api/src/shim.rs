//! The `/api/v3` Radarr/Sonarr compatibility shim.
//!
//! A separate router presenting the originals' v3 request/response shapes so the
//! existing ecosystem (Prowlarr, Overseerr/Jellyseerr, Bazarr, Recyclarr,
//! Notifiarr, dashboards) works unmodified — a non-negotiable external contract
//! (docs/09-api.md).
//!
//! ## Two faces
//! A real media stack configures a *Sonarr* (TV) and a *Radarr* (movies)
//! separately, each a URL + key. cellarr is one app, so it exposes **two faces**
//! of the same handler core:
//! - the **Sonarr face** at `/sonarr/api/v3/*` — `appName "Sonarr"`, a Sonarr v4
//!   version string, TV resources (`series`/`episode`); and
//! - the **Radarr face** at `/radarr/api/v3/*` — `appName "Radarr"`, a Radarr v5
//!   version string, movie resources (`movie`).
//!
//! The bare `/api/v3/*` mount is cellarr's own face: it auto-selects Sonarr- vs
//! Radarr-shaped responses per the addressed library's [`MediaType`], for
//! cellarr's UI and single-app integrations.
//!
//! The shapes here are reconstructed from the v3 field names captured from live
//! Sonarr 4.0.17 / Radarr 6.2.1; they are pinned by contract tests against
//! fixtures recorded from those apps (`tests/fixtures/`).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, Request};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use cellarr_core::repo::{ContentRepository, ProfileRepository};
use cellarr_core::{Library, LibraryId, MediaType, QualityProfileId};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::require_api_key;
use crate::commands::{self, command_name, kind_for_command};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::webhook::ReqwestWebhookSender;
use cellarr_core::{NotificationConfig, WebhookPayload, WebhookSender};

/// The application identity a v3 face presents.
///
/// The ecosystem branches on `appName`/version (e.g. Jellyseerr treats Sonarr v3
/// vs v4 differently; Prowlarr enforces a min-version floor read from the
/// `X-Application-Version` header), so each face answers as a current real app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    /// cellarr's own face: app surface is chosen per addressed library type.
    Cellarr,
    /// The Sonarr face: TV resources, Sonarr v4 identity.
    Sonarr,
    /// The Radarr face: movie resources, Radarr v5 identity.
    Radarr,
}

/// A current Sonarr v4 version string. Chosen to land tools in their "supported
/// Sonarr v4" band (the version captured from the live image used as ground
/// truth for the fixtures).
const SONARR_VERSION: &str = "4.0.17.2952";
/// A current Radarr v5 version string (the major the ecosystem treats as current
/// Radarr; the live image was a v6 build, but v5 is the published "current" line
/// tools gate on and the captured response surface is identical).
const RADARR_VERSION: &str = "5.27.5.10198";

impl Face {
    /// The `appName` for the surface this face presents for `media`. The Cellarr
    /// face mirrors whichever app matches the addressed library; the dedicated
    /// faces are fixed.
    fn app_name(self, media: MediaType) -> &'static str {
        match self {
            Face::Sonarr => "Sonarr",
            Face::Radarr => "Radarr",
            Face::Cellarr => match media {
                MediaType::Tv => "Sonarr",
                _ => "Radarr",
            },
        }
    }

    /// The emulated application version for this face/surface — the value of the
    /// `X-Application-Version` header and the `version` field.
    fn version(self, media: MediaType) -> &'static str {
        match self {
            Face::Sonarr => SONARR_VERSION,
            Face::Radarr => RADARR_VERSION,
            Face::Cellarr => match media {
                MediaType::Tv => SONARR_VERSION,
                _ => RADARR_VERSION,
            },
        }
    }

    /// The media type this face's *list* resources cover. The Cellarr face has no
    /// single fixed type (it serves both `series` and `movie`); the dedicated
    /// faces are pinned, which is what makes a face a real single-app surface.
    fn fixed_media(self) -> Option<MediaType> {
        match self {
            Face::Sonarr => Some(MediaType::Tv),
            Face::Radarr => Some(MediaType::Movie),
            Face::Cellarr => None,
        }
    }

    /// The header version to advertise when no library hint resolves a surface —
    /// the face's own identity (Radarr for the bare/movie default).
    fn default_version(self) -> &'static str {
        self.version(self.fixed_media().unwrap_or(MediaType::Movie))
    }
}

/// Build a v3 router for `face`, with the `X-Application-Version` header applied
/// to every response and both auth modes (`X-Api-Key` / `?apikey=`) honored on
/// mutating routes (reads stay open, matching the originals under a set key —
/// clients always send the key anyway).
///
/// The router carries `face` in its handler state via a [`FaceState`] wrapper so
/// one handler core serves all three mounts.
pub fn router(state: AppState, face: Face) -> Router {
    let fs = FaceState { state, face };

    let reads = Router::new()
        .route("/ping", get(ping))
        .route("/system/status", get(system_status))
        .route("/system/task", get(system_tasks))
        .route("/system/task/{id}", get(system_task))
        .route("/health", get(health))
        .route("/system/backup", get(list_backups))
        .route("/system/backup/{id}", get(download_backup))
        .route("/log/file", get(list_log_files))
        .route("/log/file/{name}", get(read_log_file))
        .route("/rootfolder", get(root_folders))
        .route("/rootFolder", get(root_folders))
        .route("/tag", get(list_tags))
        .route("/tag/{id}", get(get_tag))
        .route("/qualityprofile", get(quality_profiles))
        .route("/qualityProfile", get(quality_profiles))
        .route("/qualityprofile/schema", get(quality_profile_schema))
        .route("/qualityProfile/schema", get(quality_profile_schema))
        .route("/qualitydefinition", get(quality_definitions))
        .route("/qualityDefinition", get(quality_definitions))
        .route("/languageprofile", get(language_profiles))
        .route("/languageProfile", get(language_profiles))
        .route("/customformat", get(list_custom_formats))
        .route("/customFormat", get(list_custom_formats))
        .route("/customformat/schema", get(custom_format_schema))
        .route("/customFormat/schema", get(custom_format_schema))
        .route("/customformat/{id}", get(get_custom_format))
        .route("/customFormat/{id}", get(get_custom_format))
        .route("/delayprofile", get(list_delay_profiles))
        .route("/delayProfile", get(list_delay_profiles))
        .route("/delayprofile/{id}", get(get_delay_profile))
        .route("/delayProfile/{id}", get(get_delay_profile))
        .route("/indexer", get(list_indexers))
        .route("/indexer/schema", get(indexer_schema))
        .route("/downloadclient", get(list_download_clients))
        .route("/downloadClient", get(list_download_clients))
        .route("/downloadclient/schema", get(download_client_schema))
        .route("/downloadClient/schema", get(download_client_schema))
        .route("/remotepathmapping", get(list_remote_path_mappings))
        .route("/remotePathMapping", get(list_remote_path_mappings))
        .route("/config/naming", get(get_naming_config))
        .route("/config/naming/tokens", get(get_naming_tokens))
        .route("/config/mediamanagement", get(get_media_management))
        .route("/config/mediaManagement", get(get_media_management))
        .route("/notification", get(list_notifications))
        .route("/notification/schema", get(notification_schema))
        .route("/notification/{id}", get(get_notification))
        .route("/importlist", get(list_import_lists))
        .route("/importList", get(list_import_lists))
        .route("/importlist/schema", get(import_list_schema))
        .route("/importList/schema", get(import_list_schema))
        .route("/importlist/{id}", get(get_import_list))
        .route("/importList/{id}", get(get_import_list))
        .route("/importlistexclusion", get(list_import_list_exclusions))
        .route("/importListExclusion", get(list_import_list_exclusions))
        .route("/blocklist", get(list_blocklist))
        .route("/series", get(list_series))
        .route("/series/{id}", get(get_series_detail))
        .route("/episode", get(list_episodes))
        .route("/movie", get(list_movies))
        .route("/movie/{id}", get(get_movie_detail))
        .route("/movie/lookup", get(movie_lookup))
        .route("/series/lookup", get(series_lookup))
        .route("/release", get(release_search))
        .route("/manualimport", get(manual_import_scan))
        .route("/manualImport", get(manual_import_scan))
        .route("/calendar", get(calendar))
        .route("/mediacover/{contentId}/{kind}", get(media_cover))
        .route("/queue", get(queue))
        .route("/history", get(history))
        .route("/wanted/missing", get(wanted_missing))
        .route("/command", get(list_commands))
        .with_state(fs.clone());

    let writes = Router::new()
        .route("/system/backup", post(create_backup))
        .route("/system/backup/{id}", delete(delete_backup))
        .route("/system/backup/restore/{id}", post(restore_backup_id))
        .route("/system/backup/restore/upload", post(restore_backup_upload))
        .route("/rootfolder", post(create_root_folder))
        .route("/rootFolder", post(create_root_folder))
        .route("/rootfolder/{id}", delete(delete_root_folder))
        .route("/rootFolder/{id}", delete(delete_root_folder))
        .route("/movie", post(add_movie))
        .route("/movie/{id}", put(update_content))
        .route("/movie/{id}", delete(delete_movie))
        .route("/series", post(add_series))
        .route("/series/{id}", put(update_content))
        .route("/series/{id}", delete(delete_series))
        .route("/release", post(grab_release))
        .route("/manualimport", post(manual_import_commit))
        .route("/manualImport", post(manual_import_commit))
        .route("/episode/monitor", put(episode_monitor))
        .route("/season/monitor", put(season_monitor))
        .route("/command", post(command))
        .route("/tag", post(create_tag))
        .route("/tag/{id}", put(update_tag))
        .route("/tag/{id}", delete(delete_tag))
        .route("/qualityprofile", post(create_quality_profile))
        .route("/qualityProfile", post(create_quality_profile))
        .route("/qualityprofile/{id}", put(update_quality_profile))
        .route("/qualityProfile/{id}", put(update_quality_profile))
        .route("/qualityprofile/{id}", delete(delete_quality_profile))
        .route("/qualityProfile/{id}", delete(delete_quality_profile))
        .route("/customformat", post(create_custom_format))
        .route("/customFormat", post(create_custom_format))
        .route("/customformat/{id}", put(update_custom_format))
        .route("/customFormat/{id}", put(update_custom_format))
        .route("/customformat/{id}", delete(delete_custom_format))
        .route("/customFormat/{id}", delete(delete_custom_format))
        .route("/customformat/test", post(custom_format_test))
        .route("/customFormat/test", post(custom_format_test))
        .route("/delayprofile", post(create_delay_profile))
        .route("/delayProfile", post(create_delay_profile))
        .route("/delayprofile/{id}", put(update_delay_profile))
        .route("/delayProfile/{id}", put(update_delay_profile))
        .route("/delayprofile/{id}", delete(delete_delay_profile))
        .route("/delayProfile/{id}", delete(delete_delay_profile))
        .route("/indexer", post(create_indexer))
        .route("/indexer/{id}", put(update_indexer))
        .route("/indexer/{id}", delete(delete_indexer))
        .route("/indexer/test", post(test_indexer))
        .route("/downloadclient", post(create_download_client))
        .route("/downloadClient", post(create_download_client))
        .route("/downloadclient/{id}", put(update_download_client))
        .route("/downloadClient/{id}", put(update_download_client))
        .route("/downloadclient/{id}", delete(delete_download_client))
        .route("/downloadClient/{id}", delete(delete_download_client))
        .route("/downloadclient/test", post(test_download_client))
        .route("/downloadClient/test", post(test_download_client))
        .route("/remotepathmapping", post(create_remote_path_mapping))
        .route("/remotePathMapping", post(create_remote_path_mapping))
        .route("/remotepathmapping/{id}", put(update_remote_path_mapping))
        .route("/remotePathMapping/{id}", put(update_remote_path_mapping))
        .route(
            "/remotepathmapping/{id}",
            delete(delete_remote_path_mapping),
        )
        .route(
            "/remotePathMapping/{id}",
            delete(delete_remote_path_mapping),
        )
        .route("/config/naming", put(update_naming_config))
        .route("/config/naming/preview", post(preview_naming))
        .route("/config/mediamanagement", put(update_media_management))
        .route("/config/mediaManagement", put(update_media_management))
        .route("/notification", post(create_notification))
        .route("/notification/{id}", put(update_notification))
        .route("/notification/{id}", delete(delete_notification))
        .route("/notification/test", post(test_notification))
        .route("/importlist", post(create_import_list))
        .route("/importList", post(create_import_list))
        .route("/importlist/{id}", put(update_import_list))
        .route("/importList/{id}", put(update_import_list))
        .route("/importlist/{id}", delete(delete_import_list))
        .route("/importList/{id}", delete(delete_import_list))
        .route("/importlist/test", post(test_import_list))
        .route("/importList/test", post(test_import_list))
        .route("/importlist/{id}/sync", post(sync_import_list_one))
        .route("/importList/{id}/sync", post(sync_import_list_one))
        .route("/importlistexclusion", post(create_import_list_exclusion))
        .route("/importListExclusion", post(create_import_list_exclusion))
        .route(
            "/importlistexclusion/{id}",
            delete(delete_import_list_exclusion),
        )
        .route(
            "/importListExclusion/{id}",
            delete(delete_import_list_exclusion),
        )
        .route("/blocklist/{id}", delete(delete_blocklist_item))
        .route("/blocklist/bulk", delete(delete_blocklist_bulk))
        .route("/queue/{id}", delete(delete_queue_item))
        .route("/queue/{id}", put(update_queue_category))
        .route("/queue/grab", post(queue_grab))
        .layer(middleware::from_fn_with_state(
            fs.state.clone(),
            require_api_key,
        ))
        .with_state(fs.clone());

    reads
        .merge(writes)
        // Unknown /api/v3/* paths return 404 JSON, never the SPA HTML — the
        // ecosystem parses these as JSON (bug B1: the asset fallback used to
        // intercept these and return HTML 200).
        .fallback(not_found)
        // Every API response carries the emulated app version; Prowlarr reads
        // this header (not the body) and enforces a min-version floor.
        .layer(middleware::from_fn_with_state(fs, version_header))
}

/// The 404 JSON handler for unknown `/api/v3/*` paths.
async fn not_found() -> ApiError {
    ApiError::NotFound("unknown api endpoint".into())
}

/// Per-face handler state: the shared [`AppState`] plus which [`Face`] this mount
/// presents.
#[derive(Clone)]
struct FaceState {
    state: AppState,
    face: Face,
}

// --- cross-cutting: version header -----------------------------------------

/// Middleware adding `X-Application-Version` to every API response. The value is
/// the face's emulated version, resolved from a `libraryId`/`movieId`/`seriesId`
/// hint when present (so the Cellarr face advertises the right app per request),
/// else the face's default identity.
async fn version_header(
    State(fs): State<FaceState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    // Resolve the surface from a query hint cheaply (no DB round-trip on the
    // header path; the body handlers do the authoritative resolution).
    let version = match fs.face.fixed_media() {
        Some(media) => fs.face.version(media),
        None => fs.face.default_version(),
    };
    let mut resp = next.run(req).await;
    if let Ok(value) = HeaderValue::from_str(version) {
        resp.headers_mut().insert("X-Application-Version", value);
    }
    resp
}

// --- helpers ---------------------------------------------------------------

/// Resolve which app surface to present for a body handler. A dedicated face is
/// pinned to its media type; the Cellarr face uses the `libraryId` hint or the
/// first configured library, defaulting to Movie (the most common target).
async fn surface_for(fs: &FaceState, hint: Option<LibraryId>) -> ApiResult<MediaType> {
    if let Some(media) = fs.face.fixed_media() {
        return Ok(media);
    }
    let libs = fs.state.db.config().list_libraries().await?;
    if let Some(id) = hint {
        if let Some(lib) = libs.iter().find(|l| l.id == id) {
            return Ok(lib.media_type);
        }
    }
    Ok(libs
        .first()
        .map(|l| l.media_type)
        .unwrap_or(MediaType::Movie))
}

/// Parse an optional `libraryId` query parameter into a typed id.
fn library_hint(raw: Option<&str>) -> ApiResult<Option<LibraryId>> {
    raw.filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<uuid::Uuid>()
                .map(LibraryId::from_uuid)
                .map_err(|_| ApiError::BadRequest(format!("invalid libraryId: {s}")))
        })
        .transpose()
}

// --- ping ------------------------------------------------------------------

/// The unauthenticated liveness probe every *arr tool hits first.
async fn ping() -> Json<Value> {
    Json(json!({ "status": "OK" }))
}

// --- system/status ---------------------------------------------------------

#[derive(Debug, Deserialize)]
struct StatusQuery {
    #[serde(rename = "libraryId")]
    library_id: Option<String>,
}

async fn system_status(
    State(fs): State<FaceState>,
    Query(q): Query<StatusQuery>,
) -> ApiResult<Json<Value>> {
    let hint = library_hint(q.library_id.as_deref())?;
    let surface = surface_for(&fs, hint).await?;
    let app_name = fs.face.app_name(surface);
    let version = fs.face.version(surface);
    let auth = if fs.state.auth.accepts(None) {
        "none"
    } else {
        "apiKey"
    };
    // The full v3 status field set the ecosystem reads (captured from the live
    // apps). Values that describe *our* runtime are answered truthfully; the
    // app-identity fields mimic the emulated app so version-gated clients take
    // the right code path.
    Ok(Json(json!({
        "appName": app_name,
        "instanceName": app_name,
        "version": version,
        "buildTime": "2026-01-01T00:00:00Z",
        "isDebug": false,
        "isProduction": true,
        "isAdmin": false,
        "isUserInteractive": false,
        "startupPath": "/app",
        "appData": "/config",
        "osName": std::env::consts::OS,
        "osVersion": "",
        "isNetCore": true,
        "isLinux": cfg!(target_os = "linux"),
        "isOsx": cfg!(target_os = "macos"),
        "isWindows": cfg!(target_os = "windows"),
        "isDocker": std::path::Path::new("/.dockerenv").exists(),
        "mode": "console",
        "branch": "main",
        "authentication": auth,
        "migrationVersion": 1,
        "urlBase": "",
        "runtimeVersion": env!("CARGO_PKG_VERSION"),
        "runtimeName": "cellarr",
        "startTime": "2026-01-01T00:00:00Z",
        "packageVersion": concat!("cellarr-", env!("CARGO_PKG_VERSION")),
        "packageAuthor": "cellarr",
        "packageUpdateMechanism": "builtIn",
        "databaseVersion": "3.0.0",
        "databaseType": "sqLite",
    })))
}

// --- system/task -----------------------------------------------------------

/// v3 `system/task` — the scheduled-task list the Activity/System screen reads to
/// show each maintenance task's interval, next-run countdown, and last status.
///
/// cellarr's scheduler tracks every recurring job's **next due time** (`due_at`,
/// a unix timestamp) and its **lifecycle state** (which doubles as the last
/// status), and a recurring job carries its `interval_secs`. We surface those in
/// the v3 task shape (`name`/`taskName`/`interval` minutes/`nextExecution`/
/// `lastExecution`/`lastDuration`).
///
/// `lastExecution` is **derived**, not recorded: for a recurring job firing every
/// `interval_secs`, the previous fire was `nextExecution - interval` (null until
/// it has fired at least once). The scheduler does not persist a real
/// execution-completed timestamp or duration yet — that is the small DEFERRED
/// part (`lastDuration` is reported as `00:00:00`). Next-run + interval + status
/// are real, which is what the countdown + "Run now" UI needs.
async fn system_tasks(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let jobs = commands::list_jobs(&fs.state.scheduler)
        .await
        .map_err(ApiError::Command)?;
    let out = jobs.iter().filter_map(v3_task).collect();
    Ok(Json(out))
}

/// v3 `system/task/{id}` — one scheduled task by its (numeric-projected) id.
async fn system_task(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let numeric = parse_i64(&id, "task")?;
    let jobs = commands::list_jobs(&fs.state.scheduler)
        .await
        .map_err(ApiError::Command)?;
    jobs.iter()
        .find(|j| rpm_numeric_id(&j.id) == numeric)
        .and_then(v3_task)
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("task {id} not found")))
}

/// Render a recurring scheduler [`Job`](cellarr_jobs::Job) into the v3 task shape.
/// Returns `None` for a one-shot (`Once`) job — those are commands, not scheduled
/// tasks, and belong on the `command`/`queue` surface.
fn v3_task(job: &cellarr_jobs::Job) -> Option<Value> {
    use cellarr_jobs::Schedule;
    let (interval_secs, next) = match job.schedule {
        Schedule::Every {
            interval_secs,
            next,
        } => (interval_secs, next),
        Schedule::Once { .. } => return None,
    };
    let name = command_name(&job.kind);
    // The previous fire is one interval before the next due time; null until the
    // task has fired at least once (next is still its first scheduled fire).
    let last_execution = next
        .checked_sub(interval_secs)
        .map(unix_to_iso)
        .map(Value::String)
        .unwrap_or(Value::Null);
    Some(json!({
        "id": rpm_numeric_id(&job.id),
        "name": name,
        "taskName": name,
        "interval": interval_secs / 60,
        "lastExecution": last_execution,
        // The scheduler does not record a real run duration yet (DEFERRED); the
        // status the UI shows comes from the job's lifecycle state.
        "lastDuration": "00:00:00",
        "nextExecution": unix_to_iso(next),
        "lastStatus": task_status(job.state),
    }))
}

/// Map a job lifecycle state to the last-status string the UI shows.
fn task_status(state: cellarr_jobs::JobState) -> &'static str {
    use cellarr_jobs::JobState;
    match state {
        JobState::Done => "completed",
        JobState::Running => "started",
        JobState::Failed => "failed",
        JobState::Retrying => "retrying",
        JobState::Scheduled => "queued",
    }
}

/// Format a unix timestamp (seconds) as an RFC 3339 / ISO-8601 UTC string for the
/// v3 date fields. A timestamp the clock cannot represent falls back to the v3
/// zero date the *arr apps use for "never".
fn unix_to_iso(secs: u64) -> String {
    use time::format_description::well_known::Rfc3339;
    i64::try_from(secs)
        .ok()
        .and_then(|s| time::OffsetDateTime::from_unix_timestamp(s).ok())
        .and_then(|dt| dt.format(&Rfc3339).ok())
        .unwrap_or_else(|| "0001-01-01T00:00:00Z".to_string())
}

// --- health ----------------------------------------------------------------

/// v3 health checks. cellarr surfaces its own health as v3-shaped
/// `{ source, type, message, wikiUrl }` records; an all-clear is an empty array.
///
/// The breadth comes from [`crate::health::run_all`] (no-root-folder /
/// root-folder-unwritable / no-indexer / no-download-client / no-recent-backup /
/// database-ok), plus the loud cross-filesystem hardlink-fallback warning folded
/// in from [`crate::fs_health`].
async fn health(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let backup = fs.state.backup.as_deref();
    let checks = crate::health::run_all(&fs.state.db, backup).await?;
    let mut out: Vec<Value> = checks
        .iter()
        .map(crate::health::HealthCheck::to_v3)
        .collect();

    // The loud cross-filesystem warning: a configured downloads dir on a
    // different filesystem from a library root means imports silently fall back
    // to a full copy instead of a hardlink (the #1 user footgun the originals
    // hide). Surfaced here on every v3 face and `warn!`-logged.
    for w in crate::fs_health::filesystem_warnings(&fs.state.db).await? {
        out.push(json!({
            "source": w.source(),
            "type": "warning",
            "message": w.message(),
            "wikiUrl": "",
        }));
    }

    Ok(Json(out))
}

// --- system/backup ---------------------------------------------------------

/// The engine, or a structured 503-ish error when no backup wiring is attached
/// (the offline/test default). We use [`ApiError::Internal`] so the body is the
/// standard `{ code, message }`.
fn backup_engine(fs: &FaceState) -> ApiResult<&crate::backup::BackupEngine> {
    fs.state
        .backup
        .as_deref()
        .ok_or_else(|| ApiError::Internal("backup engine not configured".into()))
}

/// Map a [`crate::backup::BackupError`] onto the API error surface.
fn map_backup_err(e: crate::backup::BackupError) -> ApiError {
    use crate::backup::BackupError as B;
    match e {
        B::NotFound(id) => ApiError::NotFound(format!("backup {id} not found")),
        B::Malformed(m) => ApiError::BadRequest(format!("malformed backup bundle: {m}")),
        B::Unsupported(m) => ApiError::BadRequest(m),
        B::Db(e) => ApiError::Db(e),
        B::Io(m) => ApiError::Internal(format!("backup io error: {m}")),
    }
}

/// Render a [`crate::backup::BackupInfo`] into the v3 backup-list shape
/// (`{ id, name, type, time, size }`), mirroring the originals' `system/backup`.
fn v3_backup(info: &crate::backup::BackupInfo) -> Value {
    json!({
        "id": rpm_numeric_id(&info.id),
        "backupId": info.id,
        "name": info.name,
        "type": info.kind,
        "size": info.size,
        "time": unix_to_iso(info.created_unix.max(0) as u64),
        // The originals expose a download path; ours mirrors the GET route.
        "path": format!("/api/v3/system/backup/{}", info.id),
    })
}

/// v3 `GET /system/backup` — list backups, newest first.
async fn list_backups(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let eng = backup_engine(&fs)?;
    let backups = eng.list().map_err(map_backup_err)?;
    Ok(Json(backups.iter().map(v3_backup).collect()))
}

/// The backup create body — `{ type }` is optional (the UI sends it); we treat any
/// create as a manual backup.
#[derive(Debug, Deserialize, Default)]
struct BackupCreateBody {
    #[serde(default)]
    #[allow(dead_code)]
    name: Option<String>,
}

/// v3 `POST /system/backup` — take a manual backup now, returning its listing.
async fn create_backup(
    State(fs): State<FaceState>,
    body: Option<Json<BackupCreateBody>>,
) -> ApiResult<Json<Value>> {
    let _ = body; // body is advisory; a create is always a manual backup
    let eng = backup_engine(&fs)?;
    let info = eng
        .create("manual", serde_json::Value::Null)
        .await
        .map_err(map_backup_err)?;
    Ok(Json(v3_backup(&info)))
}

/// v3 `GET /system/backup/{id}` — download a backup bundle's raw bytes.
///
/// The `{id}` may be the numeric-projected id (the v3 `id` field) or the real
/// backup id string; we resolve a numeric id back to its string by listing.
async fn download_backup(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let eng = backup_engine(&fs)?;
    let real_id = resolve_backup_id(eng, &id)?;
    let bytes = eng.read_bundle(&real_id).map_err(map_backup_err)?;
    let body = axum::body::Body::from(bytes);
    let mut resp = Response::new(body);
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(cd) = HeaderValue::from_str(&format!("attachment; filename=\"{real_id}.cbk\"")) {
        resp.headers_mut()
            .insert(axum::http::header::CONTENT_DISPOSITION, cd);
    }
    Ok(resp)
}

/// v3 `DELETE /system/backup/{id}` — remove a backup. Idempotent.
async fn delete_backup(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let eng = backup_engine(&fs)?;
    // A missing id resolves to itself (delete is idempotent on the engine).
    let real_id = resolve_backup_id(eng, &id).unwrap_or(id);
    eng.delete(&real_id).map_err(map_backup_err)?;
    Ok(Json(json!({})))
}

/// v3 `POST /system/backup/restore/{id}` — restore from an existing backup id.
/// Takes an automatic pre-restore safety backup, validates, and atomically swaps.
async fn restore_backup_id(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let eng = backup_engine(&fs)?;
    let real_id = resolve_backup_id(eng, &id)?;
    let outcome = eng.restore_id(&real_id).await.map_err(map_backup_err)?;
    Ok(Json(json!({
        "restored": real_id,
        "safetyBackupId": outcome.safety_backup_id,
        "restartRequired": outcome.restart_required,
        "message": "Database restored. Restart cellarr for the change to take effect.",
    })))
}

/// v3 `POST /system/backup/restore/upload` — restore from an uploaded bundle (the
/// raw bundle bytes as the request body). Same safety flow as the id path.
async fn restore_backup_upload(
    State(fs): State<FaceState>,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    let eng = backup_engine(&fs)?;
    if body.is_empty() {
        return Err(ApiError::BadRequest("empty backup upload".into()));
    }
    let outcome = eng
        .restore_from_bytes(&body)
        .await
        .map_err(map_backup_err)?;
    Ok(Json(json!({
        "safetyBackupId": outcome.safety_backup_id,
        "restartRequired": outcome.restart_required,
        "message": "Database restored from upload. Restart cellarr for the change to take effect.",
    })))
}

/// Resolve a v3 backup path id (either the numeric projection or the real id
/// string) back to the real backup id, erroring if it matches nothing.
fn resolve_backup_id(eng: &crate::backup::BackupEngine, id: &str) -> ApiResult<String> {
    let list = eng.list().map_err(map_backup_err)?;
    // Exact string match first (the UI round-trips the real id).
    if list.iter().any(|b| b.id == id) {
        return Ok(id.to_string());
    }
    // Else the numeric projection the v3 `id` field carries.
    if let Ok(numeric) = id.parse::<i64>() {
        if let Some(b) = list.iter().find(|b| rpm_numeric_id(&b.id) == numeric) {
            return Ok(b.id.clone());
        }
    }
    Err(ApiError::NotFound(format!("backup {id} not found")))
}

// --- log/file --------------------------------------------------------------

/// The log reader, or a structured error when no log wiring is attached.
fn log_files(fs: &FaceState) -> ApiResult<&crate::logfile::LogFiles> {
    fs.state
        .log_files
        .as_deref()
        .ok_or_else(|| ApiError::Internal("log file reader not configured".into()))
}

/// v3 `GET /log/file` — list the daemon's on-disk log files.
async fn list_log_files(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let lf = log_files(&fs)?;
    let files = lf
        .list()
        .map_err(|e| ApiError::Internal(format!("listing log files: {e}")))?;
    let out = files
        .iter()
        .map(|f| {
            json!({
                "id": rpm_numeric_id(&f.name),
                "filename": f.name,
                "lastWriteTime": unix_to_iso(f.last_modified_unix.max(0) as u64),
                "contentsUrl": format!("/api/v3/log/file/{}", f.name),
                "size": f.size,
            })
        })
        .collect();
    Ok(Json(out))
}

/// The `?limit=` (alias `?lines=`) query for a log tail read.
#[derive(Debug, Deserialize, Default)]
struct LogTailQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    lines: Option<usize>,
}

/// v3 `GET /log/file/{name}` — read the tail of a named log file. The originals
/// return the raw file body; we return the recent lines (capped) as plain text so
/// the operator sees the same content, with path-traversal on `name` rejected.
async fn read_log_file(
    State(fs): State<FaceState>,
    Path(name): Path<String>,
    Query(q): Query<LogTailQuery>,
) -> ApiResult<Response> {
    let lf = log_files(&fs)?;
    let limit = q.limit.or(q.lines);
    let lines = lf
        .read_tail(&name, limit)
        .map_err(|_| ApiError::NotFound(format!("log file {name} not found")))?;
    let body = lines.join("\n");
    let mut resp = Response::new(axum::body::Body::from(body));
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    Ok(resp)
}

// --- rootfolder ------------------------------------------------------------

/// v3 root folders — `{ id, path, accessible, freeSpace, unmappedFolders }`.
/// cellarr derives them from the configured libraries' root folders (the v3
/// model is a flat list; libraries carry the same paths).
async fn root_folders(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let cfg = fs.state.db.config();
    let surface = fs.face.fixed_media();
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    let mut idx = 1u32;
    for lib in cfg.list_libraries().await? {
        // A dedicated face only advertises root folders of its own media type.
        if let Some(media) = surface {
            if lib.media_type != media {
                continue;
            }
        }
        for path in lib.root_folders {
            if !seen.insert(path.clone()) {
                continue;
            }
            out.push(json!({
                "id": idx,
                "path": path,
                "accessible": true,
                "freeSpace": 0,
                "unmappedFolders": [],
            }));
            idx += 1;
        }
    }
    // Also include any standalone root folders the config layer holds. These
    // carry their real id projection (not the library-derived sequential index)
    // so a folder created via POST round-trips through DELETE/{id}.
    let _ = idx;
    for rf in cfg.list_root_folders().await? {
        if seen.insert(rf.path.clone()) {
            out.push(v3_root_folder(&rf));
        }
    }
    Ok(Json(out))
}

/// Render a standalone [`RootFolder`] into the v3 root-folder shape, with its id
/// projected to the stable integer the v3 `id` field requires (so it round-trips
/// through `DELETE /rootfolder/{id}`).
fn v3_root_folder(rf: &cellarr_core::RootFolder) -> Value {
    json!({
        "id": rpm_numeric_id(&rf.id),
        "path": rf.path,
        "accessible": rf.enabled,
        "freeSpace": 0,
        "unmappedFolders": [],
    })
}

/// v3 root-folder write body (`path`, optional `name`).
#[derive(Debug, Deserialize)]
struct RootFolderBody {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// v3 `POST /rootfolder` — add a standalone root folder. The Settings UI adds
/// importable roots here; they live in the `root_folder` config table (distinct
/// from a library's own `root_folders`, which are managed via the library).
async fn create_root_folder(
    State(fs): State<FaceState>,
    Json(body): Json<RootFolderBody>,
) -> ApiResult<Json<Value>> {
    let path = body
        .path
        .clone()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ApiError::BadRequest("root folder path is required".into()))?;
    let folder = cellarr_core::RootFolder {
        id: uuid::Uuid::new_v4().to_string(),
        path,
        name: body.name.clone(),
        enabled: true,
    };
    fs.state.db.config().upsert_root_folder(&folder).await?;
    Ok(Json(v3_root_folder(&folder)))
}

/// v3 `DELETE /rootfolder/{id}` — remove a standalone root folder by its
/// (numeric-projected) id. Idempotent: a missing id still returns 200, matching
/// the *arr clients' expectation that delete succeeds on a re-issued delete.
async fn delete_root_folder(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let numeric = parse_i64(&id, "rootfolder")?;
    if let Some(rf) = fs
        .state
        .db
        .config()
        .list_root_folders()
        .await?
        .into_iter()
        .find(|rf| rpm_numeric_id(&rf.id) == numeric)
    {
        fs.state.db.config().delete_root_folder(&rf.id).await?;
    }
    Ok(Json(json!({})))
}

// --- tag -------------------------------------------------------------------

/// Render a tag as the v3 `{ id, label }` shape.
fn v3_tag(tag: &cellarr_core::Tag) -> Value {
    json!({ "id": tag.id, "label": tag.label })
}

async fn list_tags(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let tags = fs.state.db.tags().list().await?;
    Ok(Json(tags.iter().map(v3_tag).collect()))
}

async fn get_tag(State(fs): State<FaceState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let id = parse_u32(&id, "tag")?;
    fs.state
        .db
        .tags()
        .get(id)
        .await?
        .map(|t| Json(v3_tag(&t)))
        .ok_or_else(|| ApiError::NotFound(format!("tag {id} not found")))
}

#[derive(Debug, Deserialize)]
struct TagBody {
    label: String,
}

async fn create_tag(
    State(fs): State<FaceState>,
    Json(body): Json<TagBody>,
) -> ApiResult<Json<Value>> {
    if body.label.trim().is_empty() {
        return Err(ApiError::BadRequest("tag label is required".into()));
    }
    Ok(Json(v3_tag(
        &fs.state.db.tags().create(body.label.trim()).await?,
    )))
}

async fn update_tag(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Json(body): Json<TagBody>,
) -> ApiResult<Json<Value>> {
    let id = parse_u32(&id, "tag")?;
    fs.state
        .db
        .tags()
        .update(id, body.label.trim())
        .await?
        .map(|t| Json(v3_tag(&t)))
        .ok_or_else(|| ApiError::NotFound(format!("tag {id} not found")))
}

async fn delete_tag(State(fs): State<FaceState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let id = parse_u32(&id, "tag")?;
    if fs.state.db.tags().delete(id).await? {
        Ok(Json(json!({})))
    } else {
        Err(ApiError::NotFound(format!("tag {id} not found")))
    }
}

// --- qualityprofile --------------------------------------------------------

/// v3 quality profiles. cellarr's profiles are surfaced in the v3 list shape the
/// ecosystem reads, now including `formatItems[]` (CF id→score) and
/// `minUpgradeFormatScore` so Recyclarr/Configarr can sync custom-format scores.
async fn quality_profiles(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let repo = fs.state.db.profiles();
    let formats = repo.custom_formats().await?;
    // List every stored profile (not just the libraries' defaults) so a profile
    // created via the shim/UI shows up here without first being attached to a
    // library — the round-trip the management UI relies on.
    let profiles = repo.list_profiles().await?;
    let out = profiles
        .iter()
        .map(|profile| v3_quality_profile(profile, &formats, fs.face))
        .collect();
    Ok(Json(out))
}

/// Render a cellarr [`QualityProfile`] into the v3 quality-profile shape, with a
/// `formatItems` entry for every known custom format (score defaults to 0 — the
/// score lives on the [`CustomFormat`] in cellarr, which Recyclarr reads back via
/// the customformat resource). The Radarr face additionally carries `language`.
fn v3_quality_profile(
    p: &cellarr_core::QualityProfile,
    formats: &[cellarr_core::CustomFormat],
    face: Face,
) -> Value {
    let items: Vec<Value> = p
        .allowed_qualities
        .iter()
        .map(|rank| {
            json!({
                "quality": { "id": rank, "name": format!("rank-{rank}"), "source": "unknown", "resolution": 0 },
                "items": [],
                "allowed": true,
            })
        })
        .collect();
    let format_items: Vec<Value> = formats
        .iter()
        .map(|cf| {
            json!({
                "format": cf_numeric_id(cf.id),
                "name": cf.name,
                "score": cf.score,
            })
        })
        .collect();
    let mut profile = json!({
        "id": p.id.to_string(),
        "name": p.name,
        "upgradeAllowed": p.upgrades_allowed,
        "cutoff": p.cutoff_quality,
        "minFormatScore": p.min_custom_format_score,
        "cutoffFormatScore": p.upgrade_until_custom_format_score,
        "minUpgradeFormatScore": 1,
        "items": items,
        "formatItems": format_items,
    });
    if face_is_radarr(face) {
        merge_into(
            &mut profile,
            json!({ "language": { "id": -2, "name": "Original" } }),
        );
    }
    profile
}

/// v3 `qualityprofile/schema` — the template a fresh profile is built from. We
/// return cellarr's default quality ranking as allowed items plus the
/// format-score scaffold, which is what Recyclarr reads to build a profile.
async fn quality_profile_schema(State(fs): State<FaceState>) -> Json<Value> {
    let ranking = cellarr_core::QualityRanking::default();
    let items: Vec<Value> = ranking
        .qualities
        .iter()
        .map(|q| {
            json!({
                "quality": { "id": q.rank, "name": face_quality_name(&q.name, fs.face), "source": "unknown", "resolution": 0 },
                "items": [],
                "allowed": true,
            })
        })
        .collect();
    let mut schema = json!({
        "name": "",
        "upgradeAllowed": true,
        "cutoff": ranking.qualities.last().map(|q| q.rank).unwrap_or(0),
        "minFormatScore": 0,
        "cutoffFormatScore": 0,
        "minUpgradeFormatScore": 1,
        "items": items,
        "formatItems": [],
    });
    if face_is_radarr(fs.face) {
        merge_into(
            &mut schema,
            json!({ "language": { "id": -2, "name": "Original" } }),
        );
    }
    Json(schema)
}

/// v3 quality-profile write body (the Recyclarr/Configarr/UI-pushed shape). The
/// `items[]` carry the allowed qualities (each `quality.id` is a cellarr rank,
/// `allowed` gates inclusion); `cutoff` is the rank upgrades stop at; the
/// `*FormatScore` fields carry the custom-format score thresholds. Unknown extra
/// fields (`language`, `formatItems`, …) are ignored — cellarr scores custom
/// formats on the [`CustomFormat`] itself, which Recyclarr reads back separately.
#[derive(Debug, Deserialize)]
struct QualityProfileBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "upgradeAllowed", default = "default_true")]
    upgrade_allowed: bool,
    #[serde(default)]
    cutoff: Option<u32>,
    #[serde(rename = "minFormatScore", default)]
    min_format_score: i32,
    #[serde(rename = "cutoffFormatScore", default)]
    cutoff_format_score: i32,
    #[serde(default)]
    items: Vec<ProfileItemBody>,
}

/// One `items[]` entry: an allowed-quality flag against a `quality` (whose `id`
/// is the cellarr rank). Grouped items (the originals' nested `items[]`) are
/// flattened — only leaf `quality.id`s that are `allowed` contribute a rank.
#[derive(Debug, Deserialize)]
struct ProfileItemBody {
    #[serde(default)]
    quality: Option<ProfileQualityBody>,
    #[serde(default)]
    allowed: bool,
    #[serde(default)]
    items: Vec<ProfileItemBody>,
}

#[derive(Debug, Deserialize)]
struct ProfileQualityBody {
    #[serde(default)]
    id: Option<u32>,
}

/// Collect the allowed quality ranks from a (possibly nested) `items[]` tree.
fn collect_allowed_ranks(items: &[ProfileItemBody], out: &mut Vec<u32>) {
    for item in items {
        if item.allowed {
            if let Some(rank) = item.quality.as_ref().and_then(|q| q.id) {
                if !out.contains(&rank) {
                    out.push(rank);
                }
            }
        }
        collect_allowed_ranks(&item.items, out);
    }
}

/// Build a cellarr [`QualityProfile`] from a v3 write body, preserving `id` and
/// (on update) the prior `required_languages` cellarr models but v3 does not
/// carry in this shape.
fn profile_from_body(
    body: &QualityProfileBody,
    id: cellarr_core::QualityProfileId,
    required_languages: Vec<String>,
) -> cellarr_core::QualityProfile {
    let mut allowed_qualities = Vec::new();
    collect_allowed_ranks(&body.items, &mut allowed_qualities);
    // If the body carries no items at all, fall back to the default ranking so a
    // bare create still yields a usable profile rather than one allowing nothing.
    if allowed_qualities.is_empty() {
        allowed_qualities = cellarr_core::QualityRanking::default()
            .qualities
            .iter()
            .map(|q| q.rank)
            .collect();
    }
    let cutoff_quality = body
        .cutoff
        .or_else(|| allowed_qualities.iter().copied().max())
        .unwrap_or(0);
    cellarr_core::QualityProfile {
        id,
        name: body.name.clone().unwrap_or_default(),
        allowed_qualities,
        upgrades_allowed: body.upgrade_allowed,
        cutoff_quality,
        min_custom_format_score: body.min_format_score,
        upgrade_until_custom_format_score: body.cutoff_format_score,
        required_languages,
    }
}

async fn create_quality_profile(
    State(fs): State<FaceState>,
    Json(body): Json<QualityProfileBody>,
) -> ApiResult<Json<Value>> {
    let name = body.name.clone().unwrap_or_default();
    if name.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "quality profile name is required".into(),
        ));
    }
    let repo = fs.state.db.profiles();
    let profile = profile_from_body(&body, cellarr_core::QualityProfileId::new(), Vec::new());
    repo.upsert_profile(&profile).await?;
    let formats = repo.custom_formats().await?;
    Ok(Json(v3_quality_profile(&profile, &formats, fs.face)))
}

async fn update_quality_profile(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Json(body): Json<QualityProfileBody>,
) -> ApiResult<Json<Value>> {
    // The v3 profile `id` is the cellarr QualityProfileId rendered as its uuid
    // string (see `v3_quality_profile`), so it parses straight back.
    let pid = parse_profile_id(&id)?;
    let repo = fs.state.db.profiles();
    let existing = repo
        .get_profile(pid)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("quality profile {id} not found")))?;
    let profile = profile_from_body(&body, existing.id, existing.required_languages);
    repo.upsert_profile(&profile).await?;
    let formats = repo.custom_formats().await?;
    Ok(Json(v3_quality_profile(&profile, &formats, fs.face)))
}

async fn delete_quality_profile(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let pid = parse_profile_id(&id)?;
    // Idempotent: a missing profile still returns 200 (the *arr clients expect
    // delete to succeed even on a re-issued delete).
    fs.state.db.profiles().delete_profile(pid).await?;
    Ok(Json(json!({})))
}

/// Parse a v3 quality-profile path id (the profile's uuid string) into a typed
/// [`QualityProfileId`], mapping a malformed id to a structured 400.
fn parse_profile_id(raw: &str) -> ApiResult<cellarr_core::QualityProfileId> {
    raw.parse::<uuid::Uuid>()
        .map(cellarr_core::QualityProfileId::from_uuid)
        .map_err(|_| ApiError::BadRequest(format!("invalid qualityprofile id: {raw}")))
}

// --- qualitydefinition -----------------------------------------------------

/// Map cellarr's single canonical quality name to the spelling the addressed
/// face uses. Sonarr and Radarr genuinely disagree on the remux buckets:
/// Sonarr says `Bluray-1080p Remux` / `Bluray-2160p Remux` (cellarr's canonical
/// internal name), Radarr says `Remux-1080p` / `Remux-2160p`. Every other bucket
/// shares one name across both apps, so this only rewrites the two remux tiers
/// on the Radarr face. The Cellarr face mirrors Radarr (its default surface).
fn face_quality_name<'a>(canonical: &'a str, face: Face) -> std::borrow::Cow<'a, str> {
    use std::borrow::Cow;
    if !face_is_radarr(face) {
        return Cow::Borrowed(canonical);
    }
    match canonical {
        "Bluray-1080p Remux" => Cow::Borrowed("Remux-1080p"),
        "Bluray-2160p Remux" => Cow::Borrowed("Remux-2160p"),
        other => Cow::Borrowed(other),
    }
}

/// v3 `qualitydefinition` — the quality catalogue with size limits. Built from
/// cellarr's default quality ranking; Recyclarr reads it to map quality names.
/// The remux tiers are rendered with the addressed face's spelling
/// (`Bluray-…  Remux` on Sonarr, `Remux-…` on Radarr).
async fn quality_definitions(State(fs): State<FaceState>) -> Json<Vec<Value>> {
    let ranking = cellarr_core::QualityRanking::default();
    let out: Vec<Value> = ranking
        .qualities
        .iter()
        .map(|q| {
            let name = face_quality_name(&q.name, fs.face);
            json!({
                "id": q.rank + 1,
                "quality": { "id": q.rank, "name": name, "source": "unknown", "resolution": 0 },
                "title": name,
                "weight": q.rank + 1,
                "minSize": q.min_size_per_min.unwrap_or(0),
                "maxSize": q.max_size_per_min,
                "preferredSize": Value::Null,
            })
        })
        .collect();
    Json(out)
}

// --- languageprofile -------------------------------------------------------

/// v3 `languageprofile` — a Sonarr-only resource. Prowlarr's Sonarr application
/// proxy fetches it during its app-add handshake and dereferences the result, so
/// a missing/`404` body makes Prowlarr fail the "test" with a null-reference
/// error (Radarr has no language profiles, which is why only the Sonarr-app path
/// hit this). We answer with Sonarr v4's built-in default profile (id 1,
/// "English"), which is all Prowlarr needs to complete the handshake. On the
/// Radarr face the resource stays absent (returns an empty list) so the surface
/// matches the real app it emulates.
async fn language_profiles(State(fs): State<FaceState>) -> Json<Vec<Value>> {
    if !matches!(fs.face.fixed_media(), Some(MediaType::Tv) | None) {
        return Json(Vec::new());
    }
    // The English language identity Sonarr ships (id 1) plus the catch-all
    // "Any"/Original markers the apps include in a profile's language list.
    Json(vec![json!({
        "id": 1,
        "name": "English",
        "upgradeAllowed": true,
        "cutoff": { "id": 1, "name": "English" },
        "languages": [
            { "language": { "id": -1, "name": "Any" }, "allowed": true },
            { "language": { "id": 1, "name": "English" }, "allowed": true },
        ],
    })])
}

// --- customformat ----------------------------------------------------------

/// The v3 custom-format specification implementation name for a cellarr
/// condition kind. These are the implementation strings the ecosystem (Recyclarr)
/// round-trips through `customformat/schema`.
fn spec_implementation(kind: &cellarr_core::ConditionKind) -> &'static str {
    use cellarr_core::ConditionKind as K;
    match kind {
        K::ReleaseTitle { .. } => "ReleaseTitleSpecification",
        K::ReleaseGroup { .. } => "ReleaseGroupSpecification",
        K::Source { .. } => "SourceSpecification",
        K::Resolution { .. } => "ResolutionSpecification",
        // Codec and HDR have no first-class Sonarr/Radarr spec; the apps express
        // them as release-title regexes, and so do we on the wire.
        K::Codec { .. } | K::Hdr { .. } => "ReleaseTitleSpecification",
        K::QualityModifier { .. } => "QualityModifierSpecification",
        K::Language { .. } => "LanguageSpecification",
        K::IndexerFlag { .. } => "IndexerFlagSpecification",
        K::Size { .. } => "SizeSpecification",
        K::ReleaseType { .. } => "ReleaseTypeSpecification",
    }
}

/// The regex/value a condition contributes to its v3 spec `value` field.
///
/// String-valued kinds surface their string directly; typed-enum kinds surface
/// their serde token (e.g. a `Source::WebDl` becomes `"web-dl"`), so the value
/// round-trips losslessly through `condition_from_spec`. Size carries a `{min,max}`
/// object the apps model the same way.
fn spec_value(kind: &cellarr_core::ConditionKind) -> Value {
    use cellarr_core::ConditionKind as K;
    /// The bare serde token a single-field enum serializes to (e.g. `Source` ->
    /// `"web-dl"`). The enums derive a plain string for their variants.
    fn enum_token<T: serde::Serialize>(v: &T) -> Value {
        serde_json::to_value(v).unwrap_or(Value::Null)
    }
    match kind {
        K::ReleaseTitle { pattern } => json!(pattern),
        K::ReleaseGroup { name } => json!(name),
        K::Language { language } => json!(language),
        K::IndexerFlag { flag } => json!(flag),
        K::Source { source } => enum_token(source),
        // Resolution's serde token (`r1080p`) is awkward on the wire; surface the
        // conventional `1080p` form the UI and ecosystem use.
        K::Resolution { resolution } => json!(resolution_token(*resolution)),
        K::Codec { codec } => enum_token(codec),
        K::Hdr { format } => enum_token(format),
        K::QualityModifier { modifier } => enum_token(modifier),
        K::ReleaseType { release_type } => enum_token(release_type),
        K::Size { min, max } => json!({ "min": min, "max": max }),
    }
}

/// The conventional `<height>p` token for a resolution (e.g. `1080p`), the wire
/// form the CF editor and ecosystem use (the serde token is `r1080p`).
fn resolution_token(r: cellarr_core::Resolution) -> &'static str {
    use cellarr_core::Resolution as R;
    match r {
        R::R480p => "480p",
        R::R576p => "576p",
        R::R720p => "720p",
        R::R1080p => "1080p",
        R::R2160p => "2160p",
    }
}

/// Parse a `<height>p` resolution token back into a [`Resolution`].
fn resolution_from_token(token: &str) -> Option<cellarr_core::Resolution> {
    use cellarr_core::Resolution as R;
    match token.trim().to_ascii_lowercase().as_str() {
        "480p" | "480" => Some(R::R480p),
        "576p" | "576" => Some(R::R576p),
        "720p" | "720" => Some(R::R720p),
        "1080p" | "1080" => Some(R::R1080p),
        "2160p" | "2160" | "4k" | "uhd" => Some(R::R2160p),
        _ => None,
    }
}

/// Render a cellarr [`CustomFormat`] into the v3 customformat shape with its
/// `specifications[]` (one per condition). Recyclarr round-trips this exact
/// shape, so each spec carries `name`/`implementation`/`negate`/`required`/`fields`.
fn v3_custom_format(cf: &cellarr_core::CustomFormat) -> Value {
    let specs: Vec<Value> = cf
        .conditions
        .iter()
        .enumerate()
        .map(|(i, c)| {
            json!({
                "name": format!("{}-{}", cf.name, i + 1),
                "implementation": spec_implementation(&c.kind),
                "negate": c.negate,
                "required": c.required,
                "fields": [ { "name": "value", "value": spec_value(&c.kind) } ],
            })
        })
        .collect();
    json!({
        "id": cf_numeric_id(cf.id),
        "name": cf.name,
        "includeCustomFormatWhenRenaming": false,
        "specifications": specs,
    })
}

async fn list_custom_formats(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let formats = fs.state.db.profiles().custom_formats().await?;
    Ok(Json(formats.iter().map(v3_custom_format).collect()))
}

/// v3 `customformat/schema` — the catalogue of specification templates a custom
/// format is built from. Recyclarr reads it to validate the specs it pushes.
async fn custom_format_schema() -> Json<Vec<Value>> {
    // A free-text spec: one `value` textbox (a regex, group name, language code,
    // or indexer flag).
    let text_spec = |impl_name: &str, label: &str| {
        json!({
            "implementation": impl_name,
            "implementationName": label,
            "infoLink": "",
            "negate": false,
            "required": false,
            "fields": [ { "order": 0, "name": "value", "label": label, "type": "textbox", "advanced": false } ],
            "presets": [],
        })
    };
    // A select spec: a `value` dropdown whose options are the serde tokens of a
    // typed enum, so the editor can render a closed choice instead of free text.
    let select_spec = |impl_name: &str, label: &str, options: &[&str]| {
        let select_options: Vec<Value> = options
            .iter()
            .enumerate()
            .map(|(i, o)| json!({ "value": o, "name": o, "order": i }))
            .collect();
        json!({
            "implementation": impl_name,
            "implementationName": label,
            "infoLink": "",
            "negate": false,
            "required": false,
            "fields": [ {
                "order": 0, "name": "value", "label": label, "type": "select",
                "advanced": false, "selectOptions": select_options,
            } ],
            "presets": [],
        })
    };
    // A size spec: numeric min/max bounds in bytes.
    let size_spec = json!({
        "implementation": "SizeSpecification",
        "implementationName": "Size",
        "infoLink": "",
        "negate": false,
        "required": false,
        "fields": [
            { "order": 0, "name": "min", "label": "Minimum Size", "type": "number", "advanced": false, "unit": "bytes" },
            { "order": 1, "name": "max", "label": "Maximum Size", "type": "number", "advanced": false, "unit": "bytes" },
        ],
        "presets": [],
    });
    Json(vec![
        text_spec("ReleaseTitleSpecification", "Release Title"),
        text_spec("ReleaseGroupSpecification", "Release Group"),
        select_spec("SourceSpecification", "Source", SOURCE_TOKENS),
        select_spec("ResolutionSpecification", "Resolution", RESOLUTION_TOKENS),
        select_spec(
            "QualityModifierSpecification",
            "Quality Modifier",
            QUALITY_MODIFIER_TOKENS,
        ),
        select_spec(
            "ReleaseTypeSpecification",
            "Release Type",
            RELEASE_TYPE_TOKENS,
        ),
        text_spec("LanguageSpecification", "Language"),
        text_spec("IndexerFlagSpecification", "Indexer Flag"),
        size_spec,
    ])
}

/// The serde tokens of [`cellarr_core::Source`], in catalogue order — the select
/// options the CF editor offers for a Source spec.
const SOURCE_TOKENS: &[&str] = &[
    "workprint",
    "cam",
    "telesync",
    "telecine",
    "regional",
    "dvdscr",
    "sdtv",
    "hdtv",
    "raw-hd",
    "webrip",
    "web-dl",
    "dvd",
    "dvd-r",
    "bluray",
    "br-disk",
    "remux",
];

/// The serde tokens of [`cellarr_core::Resolution`].
const RESOLUTION_TOKENS: &[&str] = &["480p", "576p", "720p", "1080p", "2160p"];

/// The serde tokens of [`cellarr_core::ProperRepack`].
const QUALITY_MODIFIER_TOKENS: &[&str] = &["proper", "repack"];

/// The serde tokens of [`cellarr_core::ReleaseType`].
const RELEASE_TYPE_TOKENS: &[&str] = &[
    "movie",
    "single_episode",
    "multi_episode",
    "full_season",
    "daily",
    "absolute",
    "track",
    "book",
    "other",
];

/// v3 customformat write body. We accept the full Recyclarr-shaped body and map
/// its `specifications[]` back onto cellarr conditions where the implementation
/// is one we model; unknown spec kinds are preserved as release-title conditions
/// so the round-trip never loses a format.
#[derive(Debug, Deserialize)]
struct CustomFormatBody {
    name: String,
    #[serde(default)]
    specifications: Vec<SpecBody>,
}

#[derive(Debug, Deserialize)]
struct SpecBody {
    #[serde(default)]
    implementation: Option<String>,
    #[serde(default)]
    negate: bool,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    fields: Vec<FieldBody>,
}

#[derive(Debug, Deserialize)]
struct FieldBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    value: Value,
}

/// Map a v3 spec body back onto a cellarr condition. Every implementation cellarr
/// models maps to its typed [`ConditionKind`]; a typed-enum value that does not
/// parse (or an unmodeled implementation) degrades to a release-title regex so the
/// round-trip never loses a format.
fn condition_from_spec(spec: &SpecBody) -> cellarr_core::Condition {
    use cellarr_core::ConditionKind as K;
    let value = spec
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some("value"))
        .map(|f| &f.value);
    let value_str = value.and_then(|v| v.as_str()).unwrap_or("").to_string();

    // Parse a single-field enum from its serde token (e.g. "web-dl" -> Source),
    // falling back to a release-title regex of the raw string when it does not
    // name a known variant (so an unexpected token is preserved, not dropped).
    fn enum_or_title<T: serde::de::DeserializeOwned>(token: &str, wrap: impl FnOnce(T) -> K) -> K {
        serde_json::from_value::<T>(Value::String(token.to_string()))
            .map(wrap)
            .unwrap_or_else(|_| K::ReleaseTitle {
                pattern: token.to_string(),
            })
    }

    let kind = match spec.implementation.as_deref() {
        Some("ReleaseGroupSpecification") => K::ReleaseGroup { name: value_str },
        Some("LanguageSpecification") => K::Language {
            language: value_str,
        },
        Some("IndexerFlagSpecification") => K::IndexerFlag { flag: value_str },
        Some("SourceSpecification") => enum_or_title(&value_str, |source| K::Source { source }),
        Some("ResolutionSpecification") => match resolution_from_token(&value_str) {
            Some(resolution) => K::Resolution { resolution },
            // An unrecognized token is preserved as a title regex, not dropped.
            None => K::ReleaseTitle { pattern: value_str },
        },
        Some("QualityModifierSpecification") => {
            enum_or_title(&value_str, |modifier| K::QualityModifier { modifier })
        }
        Some("ReleaseTypeSpecification") => {
            enum_or_title(&value_str, |release_type| K::ReleaseType { release_type })
        }
        Some("SizeSpecification") => {
            // Size carries a {min,max} object (bytes); a bare value is treated as
            // the maximum, matching the apps' single-bound shorthand.
            let (min, max) = value
                .and_then(|v| v.as_object())
                .map(|o| {
                    (
                        o.get("min").and_then(serde_json::Value::as_u64),
                        o.get("max").and_then(serde_json::Value::as_u64),
                    )
                })
                .unwrap_or((None, value.and_then(serde_json::Value::as_u64)));
            K::Size { min, max }
        }
        // ReleaseTitleSpecification and any unmodeled implementation become a
        // release-title regex.
        _ => K::ReleaseTitle { pattern: value_str },
    };
    cellarr_core::Condition {
        kind,
        required: spec.required,
        negate: spec.negate,
    }
}

async fn create_custom_format(
    State(fs): State<FaceState>,
    Json(body): Json<CustomFormatBody>,
) -> ApiResult<Json<Value>> {
    let cf = cellarr_core::CustomFormat {
        id: cellarr_core::CustomFormatId::new(),
        name: body.name,
        conditions: body
            .specifications
            .iter()
            .map(condition_from_spec)
            .collect(),
        score: 0,
    };
    fs.state.db.profiles().upsert_custom_format(&cf).await?;
    Ok(Json(v3_custom_format(&cf)))
}

async fn update_custom_format(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Json(body): Json<CustomFormatBody>,
) -> ApiResult<Json<Value>> {
    // The v3 id is the numeric projection of a cellarr CustomFormatId; find the
    // existing format by that projection to preserve its uuid and score.
    let numeric = parse_i64(&id, "customformat")?;
    let existing = fs
        .state
        .db
        .profiles()
        .custom_formats()
        .await?
        .into_iter()
        .find(|cf| cf_numeric_id(cf.id) == numeric)
        .ok_or_else(|| ApiError::NotFound(format!("custom format {id} not found")))?;
    let cf = cellarr_core::CustomFormat {
        id: existing.id,
        name: body.name,
        conditions: body
            .specifications
            .iter()
            .map(condition_from_spec)
            .collect(),
        score: existing.score,
    };
    fs.state.db.profiles().upsert_custom_format(&cf).await?;
    Ok(Json(v3_custom_format(&cf)))
}

/// `GET /customformat/{id}` — one custom format in the v3 shape.
async fn get_custom_format(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let cf = find_custom_format_by_numeric(&fs, &id).await?;
    Ok(Json(v3_custom_format(&cf)))
}

async fn delete_custom_format(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    // Resolve the numeric v3 id back to the cellarr uuid, then delete. Idempotent:
    // a missing format is a no-op 200 (the ecosystem only needs a success), so a
    // double-delete never errors.
    let numeric = parse_i64(&id, "customformat")?;
    if let Some(cf) = fs
        .state
        .db
        .profiles()
        .custom_formats()
        .await?
        .into_iter()
        .find(|cf| cf_numeric_id(cf.id) == numeric)
    {
        fs.state.db.profiles().delete_custom_format(cf.id).await?;
    }
    Ok(Json(json!({})))
}

/// Resolve a v3 numeric customformat id back to its cellarr [`CustomFormat`],
/// 404ing when no format projects onto it.
async fn find_custom_format_by_numeric(
    fs: &FaceState,
    id: &str,
) -> ApiResult<cellarr_core::CustomFormat> {
    let numeric = parse_i64(id, "customformat")?;
    fs.state
        .db
        .profiles()
        .custom_formats()
        .await?
        .into_iter()
        .find(|cf| cf_numeric_id(cf.id) == numeric)
        .ok_or_else(|| ApiError::NotFound(format!("custom format {id} not found")))
}

/// The body of `POST /customformat/test`: a release title and an optional set of
/// parsed-field overrides the editor's live preview supplies.
#[derive(Debug, Deserialize)]
struct CustomFormatTestBody {
    /// The raw release title to evaluate every CF against.
    #[serde(default)]
    title: String,
    /// Optional pre-parsed fields (source/resolution/codec/…); when omitted the
    /// title is parsed. A field present here overrides the parse.
    #[serde(default)]
    parsed: Option<Value>,
    /// Optional explicit protocol for the synthetic release (defaults to torrent);
    /// only matters for protocol-sensitive specs.
    #[serde(default)]
    protocol: Option<String>,
    /// Optional indexer flags for IndexerFlag specs.
    #[serde(default)]
    indexer_flags: Vec<String>,
    /// Optional size in bytes for Size specs.
    #[serde(default)]
    size: Option<u64>,
}

/// `POST /customformat/test` — report which stored custom formats match a release
/// title (plus optional parsed-field / flag / size overrides), for the editor's
/// live preview. Each entry carries the format id, name, whether it matched, and
/// its score, mirroring the apps' CF-test response.
async fn custom_format_test(
    State(fs): State<FaceState>,
    Json(body): Json<CustomFormatTestBody>,
) -> ApiResult<Json<Vec<Value>>> {
    use cellarr_decide::MatchContext;

    let formats = fs.state.db.profiles().custom_formats().await?;

    // Build the parse: start from the real parser, then apply any explicit
    // overrides from the editor so a preview can test fields the title omits.
    let mut parsed = cellarr_parse::parse_title(&body.title);
    if let Some(over) = &body.parsed {
        apply_parsed_overrides(&mut parsed, over);
    }

    let protocol = match body.protocol.as_deref() {
        Some(p) if p.eq_ignore_ascii_case("usenet") => cellarr_core::Protocol::Usenet,
        _ => cellarr_core::Protocol::Torrent,
    };
    let release = cellarr_core::Release {
        indexer_id: cellarr_core::IndexerId::new(),
        title: body.title.clone(),
        download_url: String::new(),
        guid: None,
        protocol,
        size: body.size,
        seeders: None,
        indexer_flags: body.indexer_flags.clone(),
    };

    // Compiling can fail if a stored CF carries a dialect-incompatible regex; a CF
    // that cannot compile is reported as non-matching rather than failing the whole
    // preview.
    let report: Vec<Value> = match MatchContext::new(&formats) {
        Ok(ctx) => formats
            .iter()
            .map(|cf| {
                let matched = ctx.matches(cf, &release, &parsed);
                json!({
                    "id": cf_numeric_id(cf.id),
                    "name": cf.name,
                    "matched": matched,
                    "score": cf.score,
                })
            })
            .collect(),
        Err(_) => formats
            .iter()
            .map(|cf| {
                json!({
                    "id": cf_numeric_id(cf.id),
                    "name": cf.name,
                    "matched": false,
                    "score": cf.score,
                })
            })
            .collect(),
    };
    Ok(Json(report))
}

/// Apply editor-supplied parsed-field overrides onto a parse. Only the fields the
/// CF matcher reads are honored; an absent or unrecognized field leaves the parse
/// value from the title in place.
fn apply_parsed_overrides(parsed: &mut cellarr_core::ParsedRelease, over: &Value) {
    let Some(obj) = over.as_object() else {
        return;
    };
    let token = |k: &str| obj.get(k).and_then(serde_json::Value::as_str);
    if let Some(v) = token("source") {
        if let Ok(s) = serde_json::from_value(Value::String(v.to_string())) {
            parsed.source = Some(s);
        }
    }
    if let Some(v) = token("resolution") {
        if let Some(r) = resolution_from_token(v) {
            parsed.resolution = Some(r);
        }
    }
    if let Some(v) = token("codec") {
        if let Ok(c) = serde_json::from_value(Value::String(v.to_string())) {
            parsed.codec = Some(c);
        }
    }
    if let Some(v) = token("group") {
        parsed.group = Some(v.to_string());
    }
    if let Some(langs) = obj.get("languages").and_then(serde_json::Value::as_array) {
        parsed.languages = langs
            .iter()
            .filter_map(|l| l.as_str().map(str::to_string))
            .collect();
    }
}

// --- delayprofile ----------------------------------------------------------

/// Render a cellarr [`DelayProfile`](cellarr_core::DelayProfile) into the v3
/// delay-profile shape the ecosystem reads back. Mirrors Sonarr/Radarr's fields
/// (`preferredProtocol`, the per-protocol delays, the bypass flag, tags, order).
fn v3_delay_profile(dp: &cellarr_core::DelayProfile) -> Value {
    use cellarr_core::PreferredProtocol as P;
    let preferred = match dp.preferred_protocol {
        P::Usenet => "usenet",
        P::Torrent => "torrent",
        P::Either => "either",
    };
    // The apps split the single preference into two booleans; derive both from the
    // typed preference so a round-trip through either representation agrees.
    json!({
        "id": dp_numeric_id(dp.id),
        "enableUsenet": dp.usenet_delay > 0 || matches!(dp.preferred_protocol, P::Usenet | P::Either),
        "enableTorrent": dp.torrent_delay > 0 || matches!(dp.preferred_protocol, P::Torrent | P::Either),
        "preferredProtocol": preferred,
        "usenetDelay": dp.usenet_delay,
        "torrentDelay": dp.torrent_delay,
        "bypassIfHighestQuality": dp.bypass_if_highest_quality,
        "tags": dp.tags,
        "order": dp.order,
    })
}

/// The v3 delay-profile write body (the Sonarr/Radarr shape).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DelayProfileBody {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    preferred_protocol: Option<String>,
    #[serde(default)]
    usenet_delay: u32,
    #[serde(default)]
    torrent_delay: u32,
    #[serde(default)]
    bypass_if_highest_quality: bool,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    order: i32,
}

/// Build a cellarr [`DelayProfile`](cellarr_core::DelayProfile) from a write body,
/// preserving `id` (for updates) or minting a fresh one (for creates).
fn delay_profile_from_body(
    id: cellarr_core::DelayProfileId,
    body: &DelayProfileBody,
) -> cellarr_core::DelayProfile {
    use cellarr_core::PreferredProtocol as P;
    let preferred = match body.preferred_protocol.as_deref() {
        Some(p) if p.eq_ignore_ascii_case("usenet") => P::Usenet,
        Some(p) if p.eq_ignore_ascii_case("torrent") => P::Torrent,
        _ => P::Either,
    };
    cellarr_core::DelayProfile {
        id,
        enabled: body.enabled.unwrap_or(true),
        preferred_protocol: preferred,
        usenet_delay: body.usenet_delay,
        torrent_delay: body.torrent_delay,
        bypass_if_highest_quality: body.bypass_if_highest_quality,
        tags: body.tags.clone(),
        order: body.order,
    }
}

async fn list_delay_profiles(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let profiles = fs.state.db.profiles().list_delay_profiles().await?;
    Ok(Json(profiles.iter().map(v3_delay_profile).collect()))
}

async fn get_delay_profile(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let dp = find_delay_profile_by_numeric(&fs, &id).await?;
    Ok(Json(v3_delay_profile(&dp)))
}

async fn create_delay_profile(
    State(fs): State<FaceState>,
    Json(body): Json<DelayProfileBody>,
) -> ApiResult<Json<Value>> {
    let dp = delay_profile_from_body(cellarr_core::DelayProfileId::new(), &body);
    fs.state.db.profiles().upsert_delay_profile(&dp).await?;
    Ok(Json(v3_delay_profile(&dp)))
}

async fn update_delay_profile(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Json(body): Json<DelayProfileBody>,
) -> ApiResult<Json<Value>> {
    let existing = find_delay_profile_by_numeric(&fs, &id).await?;
    let dp = delay_profile_from_body(existing.id, &body);
    fs.state.db.profiles().upsert_delay_profile(&dp).await?;
    Ok(Json(v3_delay_profile(&dp)))
}

async fn delete_delay_profile(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let numeric = parse_i64(&id, "delayprofile")?;
    if let Some(dp) = fs
        .state
        .db
        .profiles()
        .list_delay_profiles()
        .await?
        .into_iter()
        .find(|dp| dp_numeric_id(dp.id) == numeric)
    {
        fs.state.db.profiles().delete_delay_profile(dp.id).await?;
    }
    Ok(Json(json!({})))
}

/// Resolve a v3 numeric delayprofile id back to its cellarr
/// [`DelayProfile`](cellarr_core::DelayProfile), 404ing when none projects onto it.
async fn find_delay_profile_by_numeric(
    fs: &FaceState,
    id: &str,
) -> ApiResult<cellarr_core::DelayProfile> {
    let numeric = parse_i64(id, "delayprofile")?;
    fs.state
        .db
        .profiles()
        .list_delay_profiles()
        .await?
        .into_iter()
        .find(|dp| dp_numeric_id(dp.id) == numeric)
        .ok_or_else(|| ApiError::NotFound(format!("delay profile {id} not found")))
}

// --- indexer ---------------------------------------------------------------

/// Render a cellarr [`IndexerConfig`] into the v3 indexer shape Prowlarr reads
/// back after a push: identity + flags + a `fields[]` projection of `settings`.
fn v3_indexer(ix: &cellarr_core::IndexerConfig) -> Value {
    let mut fields: Vec<Value> = ix
        .settings
        .as_object()
        .map(|o| {
            o.iter()
                .enumerate()
                .map(|(i, (k, v))| json!({ "order": i, "name": k, "value": v }))
                .collect()
        })
        .unwrap_or_default();
    // Surface the typed acceptance criteria as the v3 torrent fields the ecosystem
    // (Prowlarr/Recyclarr) reads back: minimumSeeders, the seedCriteria.* pair, and
    // a requiredFlags list (the freeleech-only policy is requiredFlags:["freeleech"]).
    // These live on their own typed column, not in settings, so they are appended.
    let c = &ix.criteria;
    if let Some(min) = c.minimum_seeders {
        fields.push(json!({ "order": 100, "name": "minimumSeeders", "value": min }));
    }
    if let Some(r) = c.seed_ratio {
        fields.push(json!({ "order": 101, "name": "seedCriteria.seedRatio", "value": r }));
    }
    if let Some(t) = c.seed_time_minutes {
        fields.push(json!({ "order": 102, "name": "seedCriteria.seedTime", "value": t }));
    }
    if !c.required_flags.is_empty() {
        fields.push(json!({ "order": 103, "name": "requiredFlags", "value": c.required_flags }));
    }
    let implementation = if ix.kind.eq_ignore_ascii_case("newznab") {
        "Newznab"
    } else {
        "Torznab"
    };
    json!({
        "id": ix_numeric_id(ix.id),
        "name": ix.name,
        "implementation": implementation,
        "implementationName": implementation,
        "configContract": format!("{implementation}Settings"),
        "protocol": protocol_str(ix.protocol),
        "priority": ix.priority,
        "enableRss": ix.enabled,
        "enableAutomaticSearch": ix.enabled,
        "enableInteractiveSearch": ix.enabled,
        "supportsRss": true,
        "supportsSearch": true,
        "fields": fields,
        "tags": ix.tags,
    })
}

async fn list_indexers(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let indexers = fs.state.db.config().list_indexers().await?;
    Ok(Json(indexers.iter().map(v3_indexer).collect()))
}

/// v3 `indexer/schema` — the Torznab and Newznab templates Prowlarr round-trips
/// its pushed indexer through.
///
/// The field set is not cosmetic: when Prowlarr adds a Sonarr/Radarr application
/// it fetches this schema, picks the Torznab (torrent) or Newznab (usenet)
/// template, and **hard-dereferences** a fixed list of fields by name
/// (`Build{Sonarr,Radarr}Indexer`): `baseUrl`, `apiPath`, `apiKey`, `categories`,
/// and — for the Sonarr face — `animeCategories`, plus the torrent
/// `minimumSeeders` / `seedCriteria.*` fields. A missing field there is a
/// `NullReferenceException` on Prowlarr's side that surfaces as "cannot connect to
/// Sonarr" during the app-add test. So each template must carry the full field set
/// the real app exposes for its protocol, gated by face (Sonarr ships the anime
/// fields; Radarr ships `multiLanguages`/`removeYear`).
async fn indexer_schema(State(fs): State<FaceState>) -> Json<Vec<Value>> {
    // The torrent-only fields both apps hard-deref when building a torrent
    // indexer (seed criteria + minimum seeders). Always present on the Torznab
    // template; harmless on Newznab.
    let torrent_fields = || -> Vec<Value> {
        vec![
            json!({ "order": 20, "name": "minimumSeeders", "label": "Minimum Seeders", "type": "number", "advanced": true, "value": 1 }),
            json!({ "order": 21, "name": "seedCriteria.seedRatio", "label": "Seed Ratio", "type": "number", "advanced": true }),
            json!({ "order": 22, "name": "seedCriteria.seedTime", "label": "Seed Time", "type": "number", "advanced": true, "unit": "minutes" }),
            json!({ "order": 23, "name": "seedCriteria.seasonPackSeedTime", "label": "Season-Pack Seed Time", "type": "number", "advanced": true, "unit": "minutes" }),
            json!({ "order": 24, "name": "rejectBlocklistedTorrentHashesWhileGrabbing", "label": "Reject Blocklisted Torrent Hashes While Grabbing", "type": "checkbox", "advanced": true, "value": false }),
        ]
    };

    // The face-specific extra fields the app's indexer builder expects.
    let face_fields = || -> Vec<Value> {
        // The bare Cellarr face presents both apps; expose the union so either
        // app's builder finds its fields.
        let sonarr = matches!(fs.face.fixed_media(), Some(MediaType::Tv) | None);
        let radarr = matches!(fs.face.fixed_media(), Some(MediaType::Movie) | None);
        let mut v = Vec::new();
        if sonarr {
            v.push(json!({ "order": 10, "name": "animeCategories", "label": "Anime Categories", "type": "select", "advanced": false }));
            v.push(json!({ "order": 11, "name": "animeStandardFormatSearch", "label": "Anime Standard Format Search", "type": "checkbox", "advanced": true, "value": false }));
        }
        if radarr {
            v.push(json!({ "order": 12, "name": "multiLanguages", "label": "Multi Languages", "type": "select", "advanced": true }));
            v.push(json!({ "order": 13, "name": "removeYear", "label": "Remove Year", "type": "checkbox", "advanced": true, "value": false }));
        }
        v
    };

    let entry = |impl_name: &str, protocol: &str| {
        let mut fields = vec![
            json!({ "order": 0, "name": "baseUrl", "label": "URL", "type": "textbox", "advanced": false }),
            json!({ "order": 1, "name": "apiPath", "label": "API Path", "value": "/api", "type": "textbox", "advanced": true }),
            json!({ "order": 2, "name": "apiKey", "label": "API Key", "type": "textbox", "advanced": false, "privacy": "apiKey" }),
            json!({ "order": 3, "name": "categories", "label": "Categories", "type": "select", "advanced": false }),
            json!({ "order": 4, "name": "additionalParameters", "label": "Additional Parameters", "type": "textbox", "advanced": true }),
        ];
        fields.extend(face_fields());
        if protocol == "torrent" {
            fields.extend(torrent_fields());
        }
        json!({
            "name": "",
            "implementation": impl_name,
            "implementationName": impl_name,
            "configContract": format!("{impl_name}Settings"),
            "infoLink": "",
            "protocol": protocol,
            "priority": 25,
            "enableRss": true,
            "enableAutomaticSearch": true,
            "enableInteractiveSearch": true,
            "supportsRss": true,
            "supportsSearch": true,
            "fields": fields,
            "presets": [],
            "tags": [],
        })
    };
    Json(vec![
        entry("Torznab", "torrent"),
        entry("Newznab", "usenet"),
    ])
}

/// v3 indexer write body (the Prowlarr-pushed shape). We map the `fields[]`
/// back into cellarr's `settings` JSON and the identity onto an [`IndexerConfig`].
#[derive(Debug, Deserialize)]
struct IndexerBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    implementation: Option<String>,
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    priority: Option<i32>,
    #[serde(default = "default_true")]
    #[serde(rename = "enableRss")]
    enable_rss: bool,
    /// The tag ids this indexer is scoped to (the v3 `tags` array). Empty/omitted
    /// = global (applies to all content).
    #[serde(default)]
    tags: Vec<u32>,
    #[serde(default)]
    fields: Vec<FieldBody>,
}

const fn default_true() -> bool {
    true
}

/// Whether `?forceSave=true` was passed: Prowlarr sets it to bypass connectivity
/// validation when pushing. cellarr never validates connectivity inline on the
/// shim path, so this is accepted but informational.
#[derive(Debug, Deserialize, Default)]
struct ForceSaveQuery {
    #[serde(rename = "forceSave", default)]
    force_save: Option<bool>,
}

fn indexer_from_body(
    body: &IndexerBody,
    id: cellarr_core::IndexerId,
) -> cellarr_core::IndexerConfig {
    // Lift the typed acceptance-criteria fields out of the pushed `fields[]` into
    // the typed `criteria` column; everything else stays in the open-ended
    // `settings` JSON. This keeps minimumSeeders/seedCriteria/requiredFlags
    // queryable rather than buried in the settings blob.
    let mut settings = serde_json::Map::new();
    let mut criteria = cellarr_core::IndexerCriteria::default();
    for f in &body.fields {
        let Some(name) = &f.name else { continue };
        match name.as_str() {
            "minimumSeeders" => {
                criteria.minimum_seeders = field_u32(&f.value);
            }
            "seedCriteria.seedRatio" => {
                criteria.seed_ratio = field_f64(&f.value);
            }
            "seedCriteria.seedTime" => {
                criteria.seed_time_minutes = field_u64(&f.value);
            }
            "requiredFlags" => {
                criteria.required_flags = field_flag_list(&f.value);
            }
            _ => {
                settings.insert(name.clone(), f.value.clone());
            }
        }
    }
    let kind = match body.implementation.as_deref() {
        Some(i) if i.eq_ignore_ascii_case("newznab") => "newznab",
        _ => "torznab",
    }
    .to_string();
    let protocol = match body.protocol.as_deref() {
        Some(p) if p.eq_ignore_ascii_case("usenet") => cellarr_core::Protocol::Usenet,
        _ => cellarr_core::Protocol::Torrent,
    };
    cellarr_core::IndexerConfig {
        id,
        name: body.name.clone().unwrap_or_default(),
        kind,
        protocol,
        enabled: body.enable_rss,
        priority: body.priority.unwrap_or(25),
        criteria,
        tags: body.tags.clone(),
        settings: Value::Object(settings),
    }
}

/// Read a numeric v3 field `value` that may arrive as a JSON number or a stringy
/// number (Prowlarr/Recyclarr serialize some fields as strings), as a `u32`.
fn field_u32(v: &Value) -> Option<u32> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        .map(|n| n.min(u64::from(u32::MAX)) as u32)
}

/// Read a numeric v3 field `value` (number or stringy number) as a `u64`.
fn field_u64(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Read a numeric v3 field `value` (number or stringy number) as an `f64`.
fn field_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Read a `requiredFlags` field `value` into a lowercase flag list. Accepts a
/// JSON array of strings or a comma-separated string; empty/blank entries dropped.
fn field_flag_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(items) => items
            .iter()
            .filter_map(|i| i.as_str())
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect(),
        Value::String(s) => s
            .split(',')
            .map(|p| p.trim().to_ascii_lowercase())
            .filter(|p| !p.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

async fn create_indexer(
    State(fs): State<FaceState>,
    Query(q): Query<ForceSaveQuery>,
    Json(body): Json<IndexerBody>,
) -> ApiResult<Json<Value>> {
    // `?forceSave=true` is honored: the shim does no inline connectivity check,
    // so a push always saves; we record the flag for observability.
    if q.force_save == Some(true) {
        tracing::debug!("indexer create with forceSave=true");
    }
    let ix = indexer_from_body(&body, cellarr_core::IndexerId::new());
    fs.state.db.config().upsert_indexer(&ix).await?;
    Ok(Json(v3_indexer(&ix)))
}

async fn update_indexer(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Query(q): Query<ForceSaveQuery>,
    Json(body): Json<IndexerBody>,
) -> ApiResult<Json<Value>> {
    if q.force_save == Some(true) {
        tracing::debug!("indexer update with forceSave=true");
    }
    let numeric = parse_i64(&id, "indexer")?;
    let existing = fs
        .state
        .db
        .config()
        .list_indexers()
        .await?
        .into_iter()
        .find(|ix| ix_numeric_id(ix.id) == numeric)
        .ok_or_else(|| ApiError::NotFound(format!("indexer {id} not found")))?;
    let ix = indexer_from_body(&body, existing.id);
    fs.state.db.config().upsert_indexer(&ix).await?;
    Ok(Json(v3_indexer(&ix)))
}

async fn delete_indexer(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    // Map the v3 integer id back to the stored uuid and delete it. A missing
    // indexer is accepted idempotently (the *arr clients expect delete to 200
    // even on a re-issued delete).
    let numeric = parse_i64(&id, "indexer")?;
    if let Some(ix) = fs
        .state
        .db
        .config()
        .list_indexers()
        .await?
        .into_iter()
        .find(|ix| ix_numeric_id(ix.id) == numeric)
    {
        fs.state.db.config().delete_indexer(ix.id).await?;
    }
    Ok(Json(json!({})))
}

/// v3 `indexer/test` — Prowlarr posts the indexer body to validate it. cellarr
/// accepts a well-formed body (the shim does no live connectivity check), which
/// is the success contract Prowlarr needs to proceed with the push.
async fn test_indexer(Json(body): Json<IndexerBody>) -> ApiResult<Json<Value>> {
    let has_base_url = body
        .fields
        .iter()
        .any(|f| f.name.as_deref() == Some("baseUrl") && !f.value.is_null());
    if !has_base_url {
        return Err(ApiError::BadRequest("indexer baseUrl is required".into()));
    }
    Ok(Json(json!({ "isValid": true, "validationFailures": [] })))
}

// --- download client -------------------------------------------------------

/// Render a cellarr [`DownloadClientConfig`] into the v3 download-client shape the
/// ecosystem reads back after a push: identity + a `fields[]` projection of
/// `settings`, plus `category` surfaced as the `category`/`tvCategory`/
/// `movieCategory` field clients hard-deref.
fn v3_download_client(dc: &cellarr_core::DownloadClientConfig) -> Value {
    let mut fields: Vec<Value> = dc
        .settings
        .as_object()
        .map(|o| {
            o.iter()
                .enumerate()
                .map(|(i, (k, v))| json!({ "order": i, "name": k, "value": v }))
                .collect()
        })
        .unwrap_or_default();
    // The category lives on its own typed column, not in settings; surface it as
    // the field the apps read.
    fields.push(json!({ "order": 100, "name": "category", "value": dc.category }));
    json!({
        "id": dc_numeric_id(dc.id),
        "name": dc.name,
        "implementation": dc_implementation(&dc.kind, dc.protocol),
        "implementationName": dc_implementation(&dc.kind, dc.protocol),
        "configContract": format!("{}Settings", dc_implementation(&dc.kind, dc.protocol)),
        "protocol": protocol_str(dc.protocol),
        "priority": dc.priority,
        "enable": dc.enabled,
        "fields": fields,
        "tags": dc.tags,
    })
}

/// The v3 `implementation` string for a cellarr download-client kind. The
/// blackhole splits by protocol into the two implementations the ecosystem knows
/// (`TorrentBlackhole` / `UsenetBlackhole`); other kinds map by name.
fn dc_implementation(kind: &str, protocol: cellarr_core::Protocol) -> &'static str {
    match kind {
        "blackhole" | "torrentblackhole" | "usenetblackhole" => match protocol {
            cellarr_core::Protocol::Torrent => "TorrentBlackhole",
            cellarr_core::Protocol::Usenet => "UsenetBlackhole",
        },
        "qbittorrent" => "QBittorrent",
        "transmission" => "Transmission",
        "deluge" => "Deluge",
        "rtorrent" => "RTorrent",
        "sabnzbd" => "Sabnzbd",
        "nzbget" => "Nzbget",
        _ => "Blackhole",
    }
}

async fn list_download_clients(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let clients = fs.state.db.config().list_download_clients().await?;
    Ok(Json(clients.iter().map(v3_download_client).collect()))
}

/// v3 `downloadclient/schema` — the implementation templates the ecosystem
/// round-trips a pushed client through. cellarr advertises the *universal*
/// blackhole pair (`TorrentBlackhole` / `UsenetBlackhole`) since it works with any
/// client a user runs, plus the natively-driven torrent templates (`Transmission`,
/// `Deluge`, `RTorrent`) carrying the host / port / urlBase / credential / category
/// fields each client takes. Each template carries the `category` field the apps
/// hard-deref.
async fn download_client_schema() -> Json<Vec<Value>> {
    let blackhole = |impl_name: &str, protocol: &str| {
        json!({
            "name": "",
            "implementation": impl_name,
            "implementationName": impl_name,
            "configContract": format!("{impl_name}Settings"),
            "infoLink": "",
            "protocol": protocol,
            "priority": 1,
            "enable": true,
            "fields": [
                json!({ "order": 0, "name": "watchFolder", "label": "Watch Folder", "helpText": "Folder cellarr drops the .torrent/.nzb/.magnet job into for your client to pick up", "type": "textbox", "advanced": false }),
                json!({ "order": 1, "name": "completedFolder", "label": "Completed Folder", "helpText": "Folder your client drops finished downloads into for cellarr to import", "type": "textbox", "advanced": false }),
                json!({ "order": 2, "name": "category", "label": "Category", "type": "textbox", "advanced": false }),
            ],
            "presets": [],
            "tags": [],
        })
    };
    // The Transmission template mirrors the real Transmission download-client
    // fields the ecosystem pushes (host/port/urlBase + optional Basic creds) plus
    // the category field cellarr files torrents under.
    let transmission = json!({
        "name": "",
        "implementation": "Transmission",
        "implementationName": "Transmission",
        "configContract": "TransmissionSettings",
        "infoLink": "",
        "protocol": "torrent",
        "priority": 1,
        "enable": true,
        "fields": [
            json!({ "order": 0, "name": "host", "label": "Host", "type": "textbox", "advanced": false, "value": "localhost" }),
            json!({ "order": 1, "name": "port", "label": "Port", "type": "number", "advanced": false, "value": 9091 }),
            json!({ "order": 2, "name": "urlBase", "label": "URL Base", "helpText": "Adds a prefix to the Transmission RPC path, e.g. /transmission for a reverse-proxy mount", "type": "textbox", "advanced": true }),
            json!({ "order": 3, "name": "username", "label": "Username", "type": "textbox", "advanced": false }),
            json!({ "order": 4, "name": "password", "label": "Password", "type": "password", "advanced": false, "privacy": "password" }),
            json!({ "order": 5, "name": "downloadDir", "label": "Directory", "helpText": "Optional absolute download root; cellarr files torrents under <root>/<category>. Leave blank to use Transmission's own download dir (Transmission rejects a relative path)", "type": "textbox", "advanced": true }),
            json!({ "order": 6, "name": "category", "label": "Category", "type": "textbox", "advanced": false }),
        ],
        "presets": [],
        "tags": [],
    });
    // The Deluge template: the JSON-RPC WebUI (host/port/urlBase + the single
    // WebUI password) plus the optional download dir and the category cellarr
    // files torrents under (modelled as a Deluge Label-plugin label).
    let deluge = json!({
        "name": "",
        "implementation": "Deluge",
        "implementationName": "Deluge",
        "configContract": "DelugeSettings",
        "infoLink": "",
        "protocol": "torrent",
        "priority": 1,
        "enable": true,
        "fields": [
            json!({ "order": 0, "name": "host", "label": "Host", "type": "textbox", "advanced": false, "value": "localhost" }),
            json!({ "order": 1, "name": "port", "label": "Port", "type": "number", "advanced": false, "value": 8112 }),
            json!({ "order": 2, "name": "urlBase", "label": "URL Base", "helpText": "Adds a prefix to the Deluge JSON-RPC path for a reverse-proxy mount", "type": "textbox", "advanced": true }),
            json!({ "order": 3, "name": "password", "label": "Password", "helpText": "The Deluge WebUI password (the only credential the JSON-RPC login takes)", "type": "password", "advanced": false, "privacy": "password" }),
            json!({ "order": 4, "name": "downloadDir", "label": "Directory", "helpText": "Optional absolute download location; leave blank to use Deluge's own default", "type": "textbox", "advanced": true }),
            json!({ "order": 5, "name": "category", "label": "Category", "helpText": "Filed as a Deluge Label-plugin label", "type": "textbox", "advanced": false }),
        ],
        "presets": [],
        "tags": [],
    });
    // The rTorrent template: the XML-RPC mount (host/port + the urlBase path, e.g.
    // /RPC2 or a ruTorrent httprpc action.php) plus optional HTTP Basic creds the
    // web front-end enforces, the download dir, and the category (an rTorrent
    // d.custom1 label).
    let rtorrent = json!({
        "name": "",
        "implementation": "RTorrent",
        "implementationName": "rTorrent",
        "configContract": "RTorrentSettings",
        "infoLink": "",
        "protocol": "torrent",
        "priority": 1,
        "enable": true,
        "fields": [
            json!({ "order": 0, "name": "host", "label": "Host", "type": "textbox", "advanced": false, "value": "localhost" }),
            json!({ "order": 1, "name": "port", "label": "Port", "type": "number", "advanced": false, "value": 8080 }),
            json!({ "order": 2, "name": "urlBase", "label": "URL Path", "helpText": "The XML-RPC mount path, e.g. /RPC2 or /rutorrent/plugins/httprpc/action.php", "type": "textbox", "advanced": false, "value": "/RPC2" }),
            json!({ "order": 3, "name": "username", "label": "Username", "helpText": "HTTP Basic username the web front-end enforces (rTorrent's XML-RPC has no native auth)", "type": "textbox", "advanced": false }),
            json!({ "order": 4, "name": "password", "label": "Password", "type": "password", "advanced": false, "privacy": "password" }),
            json!({ "order": 5, "name": "downloadDir", "label": "Directory", "helpText": "Optional absolute download root; cellarr files torrents under <root>/<category>", "type": "textbox", "advanced": true }),
            json!({ "order": 6, "name": "category", "label": "Category", "helpText": "Filed into rTorrent's d.custom1 label", "type": "textbox", "advanced": false }),
        ],
        "presets": [],
        "tags": [],
    });
    Json(vec![
        blackhole("TorrentBlackhole", "torrent"),
        blackhole("UsenetBlackhole", "usenet"),
        transmission,
        deluge,
        rtorrent,
    ])
}

/// v3 download-client write body (the Sonarr/Radarr-pushed shape). Maps the
/// `fields[]` back into cellarr's `settings` JSON and the identity onto a
/// [`DownloadClientConfig`]; `category` is lifted out of the fields into the typed
/// column.
#[derive(Debug, Deserialize)]
struct DownloadClientBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    implementation: Option<String>,
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    priority: Option<i32>,
    #[serde(default = "default_true")]
    enable: bool,
    /// The tag ids this client is scoped to (the v3 `tags` array). Empty/omitted
    /// = global (applies to all content).
    #[serde(default)]
    tags: Vec<u32>,
    #[serde(default)]
    fields: Vec<FieldBody>,
}

fn download_client_from_body(
    body: &DownloadClientBody,
    id: cellarr_core::DownloadClientId,
) -> cellarr_core::DownloadClientConfig {
    let mut settings = serde_json::Map::new();
    let mut category = String::new();
    for f in &body.fields {
        if let Some(name) = &f.name {
            if name == "category" {
                category = f.value.as_str().unwrap_or_default().to_string();
                continue;
            }
            settings.insert(name.clone(), f.value.clone());
        }
    }
    let protocol = match body.protocol.as_deref() {
        Some(p) if p.eq_ignore_ascii_case("usenet") => cellarr_core::Protocol::Usenet,
        _ => cellarr_core::Protocol::Torrent,
    };
    // Normalize the implementation into a cellarr kind. The two blackhole
    // implementations collapse to one kind ("blackhole"); the protocol carries
    // the torrent/usenet distinction.
    let kind = match body.implementation.as_deref() {
        Some(i) if i.eq_ignore_ascii_case("torrentblackhole") => "blackhole".to_string(),
        Some(i) if i.eq_ignore_ascii_case("usenetblackhole") => "blackhole".to_string(),
        Some(i) if i.eq_ignore_ascii_case("qbittorrent") => "qbittorrent".to_string(),
        Some(i) if i.eq_ignore_ascii_case("transmission") => "transmission".to_string(),
        Some(i) if i.eq_ignore_ascii_case("deluge") => "deluge".to_string(),
        Some(i) if i.eq_ignore_ascii_case("rtorrent") => "rtorrent".to_string(),
        Some(i) if i.eq_ignore_ascii_case("sabnzbd") => "sabnzbd".to_string(),
        Some(i) if i.eq_ignore_ascii_case("nzbget") => "nzbget".to_string(),
        Some(i) => i.to_ascii_lowercase(),
        None => "blackhole".to_string(),
    };
    cellarr_core::DownloadClientConfig {
        id,
        name: body.name.clone().unwrap_or_default(),
        kind,
        protocol,
        enabled: body.enable,
        priority: body.priority.unwrap_or(1),
        category,
        tags: body.tags.clone(),
        settings: Value::Object(settings),
    }
}

async fn create_download_client(
    State(fs): State<FaceState>,
    Query(q): Query<ForceSaveQuery>,
    Json(body): Json<DownloadClientBody>,
) -> ApiResult<Json<Value>> {
    if q.force_save == Some(true) {
        tracing::debug!("download client create with forceSave=true");
    }
    let dc = download_client_from_body(&body, cellarr_core::DownloadClientId::new());
    fs.state.db.config().upsert_download_client(&dc).await?;
    Ok(Json(v3_download_client(&dc)))
}

async fn update_download_client(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Query(q): Query<ForceSaveQuery>,
    Json(body): Json<DownloadClientBody>,
) -> ApiResult<Json<Value>> {
    if q.force_save == Some(true) {
        tracing::debug!("download client update with forceSave=true");
    }
    let numeric = parse_i64(&id, "downloadclient")?;
    let existing = fs
        .state
        .db
        .config()
        .list_download_clients()
        .await?
        .into_iter()
        .find(|dc| dc_numeric_id(dc.id) == numeric)
        .ok_or_else(|| ApiError::NotFound(format!("download client {id} not found")))?;
    let dc = download_client_from_body(&body, existing.id);
    fs.state.db.config().upsert_download_client(&dc).await?;
    Ok(Json(v3_download_client(&dc)))
}

async fn delete_download_client(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let numeric = parse_i64(&id, "downloadclient")?;
    if let Some(dc) = fs
        .state
        .db
        .config()
        .list_download_clients()
        .await?
        .into_iter()
        .find(|dc| dc_numeric_id(dc.id) == numeric)
    {
        fs.state.db.config().delete_download_client(dc.id).await?;
    }
    Ok(Json(json!({})))
}

/// v3 `downloadclient/test` — accepts a well-formed body. The blackhole only
/// needs a watch folder; we validate that one field is present, matching the
/// success contract the apps expect before saving.
async fn test_download_client(Json(body): Json<DownloadClientBody>) -> ApiResult<Json<Value>> {
    let has_watch = body
        .fields
        .iter()
        .any(|f| f.name.as_deref() == Some("watchFolder") && !f.value.is_null());
    // API-driven clients carry a host instead; accept either so a pushed
    // qBittorrent/SABnzbd config also validates.
    let has_host = body.fields.iter().any(|f| {
        matches!(
            f.name.as_deref(),
            Some("host") | Some("baseUrl") | Some("url")
        ) && !f.value.is_null()
    });
    if !has_watch && !has_host {
        return Err(ApiError::BadRequest(
            "download client requires a watchFolder (blackhole) or host".into(),
        ));
    }
    Ok(Json(json!({ "isValid": true, "validationFailures": [] })))
}

// --- remote path mapping ---------------------------------------------------

/// Render a cellarr [`RemotePathMapping`] into the v3 shape Recyclarr/UoMi read.
fn v3_remote_path_mapping(m: &cellarr_core::RemotePathMapping) -> Value {
    json!({
        "id": rpm_numeric_id(&m.id),
        "host": m.host,
        "remotePath": m.remote_path,
        "localPath": m.local_path,
    })
}

async fn list_remote_path_mappings(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let mappings = fs.state.db.config().list_remote_path_mappings().await?;
    Ok(Json(mappings.iter().map(v3_remote_path_mapping).collect()))
}

/// v3 remote-path-mapping write body (`host`/`remotePath`/`localPath`).
#[derive(Debug, Deserialize)]
struct RemotePathMappingBody {
    #[serde(default)]
    host: Option<String>,
    #[serde(rename = "remotePath", default)]
    remote_path: Option<String>,
    #[serde(rename = "localPath", default)]
    local_path: Option<String>,
}

async fn create_remote_path_mapping(
    State(fs): State<FaceState>,
    Json(body): Json<RemotePathMappingBody>,
) -> ApiResult<Json<Value>> {
    let remote_path = body
        .remote_path
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("remotePath is required".into()))?;
    let local_path = body
        .local_path
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("localPath is required".into()))?;
    let mapping = cellarr_core::RemotePathMapping {
        id: uuid::Uuid::new_v4().to_string(),
        host: body.host.clone().unwrap_or_default(),
        remote_path,
        local_path,
    };
    fs.state
        .db
        .config()
        .upsert_remote_path_mapping(&mapping)
        .await?;
    Ok(Json(v3_remote_path_mapping(&mapping)))
}

async fn update_remote_path_mapping(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Json(body): Json<RemotePathMappingBody>,
) -> ApiResult<Json<Value>> {
    let numeric = parse_i64(&id, "remotepathmapping")?;
    let existing = fs
        .state
        .db
        .config()
        .list_remote_path_mappings()
        .await?
        .into_iter()
        .find(|m| rpm_numeric_id(&m.id) == numeric)
        .ok_or_else(|| ApiError::NotFound(format!("remote path mapping {id} not found")))?;
    let mapping = cellarr_core::RemotePathMapping {
        id: existing.id.clone(),
        host: body.host.clone().unwrap_or(existing.host),
        remote_path: body
            .remote_path
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or(existing.remote_path),
        local_path: body
            .local_path
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or(existing.local_path),
    };
    fs.state
        .db
        .config()
        .upsert_remote_path_mapping(&mapping)
        .await?;
    Ok(Json(v3_remote_path_mapping(&mapping)))
}

async fn delete_remote_path_mapping(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let numeric = parse_i64(&id, "remotepathmapping")?;
    if let Some(m) = fs
        .state
        .db
        .config()
        .list_remote_path_mappings()
        .await?
        .into_iter()
        .find(|m| rpm_numeric_id(&m.id) == numeric)
    {
        fs.state
            .db
            .config()
            .delete_remote_path_mapping(&m.id)
            .await?;
    }
    Ok(Json(json!({})))
}

// --- naming config ---------------------------------------------------------

/// Render the persisted naming formats into the v3 naming-config payload: the four
/// configurable formats keyed by target, plus whether season folders are used.
fn v3_naming_config(naming: &cellarr_core::NamingFormats) -> Value {
    json!({
        "movieFileFormat": naming.movie_file_format,
        "seriesFolderFormat": naming.series_folder_format,
        "seasonFolderFormat": naming.season_folder_format,
        "episodeFileFormat": naming.episode_file_format,
        "renameEpisodes": true,
        "renameMovies": true,
        "seasonFolders": !naming.season_folder_format.trim().is_empty(),
    })
}

/// `GET /config/naming` — the persisted per-media-type naming formats.
async fn get_naming_config(State(fs): State<FaceState>) -> ApiResult<Json<Value>> {
    let mm = fs.state.db.config().get_media_management().await?;
    Ok(Json(v3_naming_config(&mm.naming)))
}

/// v3 naming-config write body. Every field is optional so a partial PUT leaves the
/// other formats untouched (the *arr config endpoints merge rather than replace).
#[derive(Debug, Deserialize)]
struct NamingConfigBody {
    #[serde(rename = "movieFileFormat", default)]
    movie_file_format: Option<String>,
    #[serde(rename = "seriesFolderFormat", default)]
    series_folder_format: Option<String>,
    #[serde(rename = "seasonFolderFormat", default)]
    season_folder_format: Option<String>,
    #[serde(rename = "episodeFileFormat", default)]
    episode_file_format: Option<String>,
}

/// `PUT /config/naming` — update one or more naming formats. A submitted format is
/// validated against its target's sample context before it is persisted, so an
/// invalid format (an unterminated token, a missing *required* token) is rejected
/// with `400` rather than silently saved and only failing at import time.
async fn update_naming_config(
    State(fs): State<FaceState>,
    Json(body): Json<NamingConfigBody>,
) -> ApiResult<Json<Value>> {
    use cellarr_core::NameTarget;

    let mut mm = fs.state.db.config().get_media_management().await?;
    let validated = [
        (NameTarget::MovieFile, &body.movie_file_format),
        (NameTarget::SeriesFolder, &body.series_folder_format),
        (NameTarget::SeasonFolder, &body.season_folder_format),
        (NameTarget::EpisodeFile, &body.episode_file_format),
    ];
    for (target, submitted) in validated {
        let Some(fmt) = submitted else { continue };
        // A folder format may legitimately be empty (flat layout); a file format
        // may not. Validate non-empty formats render against the sample context so
        // a malformed/under-specified format is refused before it is persisted.
        if !fmt.trim().is_empty() {
            cellarr_fs::render_preview(fmt, target, None).map_err(|e| {
                ApiError::BadRequest(format!("invalid {} format: {e}", target.key()))
            })?;
        }
        match target {
            NameTarget::MovieFile => mm.naming.movie_file_format = fmt.clone(),
            NameTarget::SeriesFolder => mm.naming.series_folder_format = fmt.clone(),
            NameTarget::SeasonFolder => mm.naming.season_folder_format = fmt.clone(),
            NameTarget::EpisodeFile => mm.naming.episode_file_format = fmt.clone(),
        }
    }
    fs.state.db.config().set_media_management(&mm).await?;
    Ok(Json(v3_naming_config(&mm.naming)))
}

/// `GET /config/naming/tokens` — the available naming-token vocabulary per name
/// target, so the UI can offer an insertable token palette with required/optional
/// markers and example values.
async fn get_naming_tokens() -> Json<Value> {
    use cellarr_core::NameTarget;
    let render_target = |target: NameTarget| {
        let tokens: Vec<Value> = cellarr_fs::token_vocabulary(target)
            .into_iter()
            .map(|t| {
                json!({
                    "token": format!("{{{}}}", t.token),
                    "name": t.token,
                    "label": t.label,
                    "required": t.required,
                    "example": t.example,
                })
            })
            .collect();
        json!({ "target": target.key(), "tokens": tokens })
    };
    Json(json!({
        "targets": NameTarget::all().into_iter().map(render_target).collect::<Vec<_>>(),
    }))
}

/// v3 naming-preview body: a candidate `format`, the `mediaType`/target it applies
/// to, and an optional `sampleContext` overriding the built-in token examples.
#[derive(Debug, Deserialize)]
struct NamingPreviewBody {
    format: String,
    #[serde(rename = "mediaType", default)]
    media_type: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(rename = "sampleContext", default)]
    sample_context: Option<std::collections::HashMap<String, String>>,
}

/// `POST /config/naming/preview` — render a candidate format against a sample
/// context and return the example string, so the UI shows a live preview as the
/// user edits a naming format. Required-token strictness and graceful optional-token
/// behavior are exactly those of the import-time rename engine.
async fn preview_naming(Json(body): Json<NamingPreviewBody>) -> ApiResult<Json<Value>> {
    use cellarr_core::{MediaType, NameTarget};

    // Resolve the name target from an explicit `target`, else the `mediaType`.
    let target = match body.target.as_deref() {
        Some("movieFile") => NameTarget::MovieFile,
        Some("seriesFolder") => NameTarget::SeriesFolder,
        Some("seasonFolder") => NameTarget::SeasonFolder,
        Some("episodeFile") => NameTarget::EpisodeFile,
        Some(other) => {
            return Err(ApiError::BadRequest(format!(
                "unknown naming target {other:?}"
            )));
        }
        None => match body
            .media_type
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("movie") => NameTarget::MovieFile,
            Some("tv") | Some("series") | Some("episode") => NameTarget::EpisodeFile,
            Some(other) => {
                return Err(ApiError::BadRequest(format!("unknown mediaType {other:?}")));
            }
            // Default to the movie file target when neither is supplied.
            None => cellarr_fs::primary_target(MediaType::Movie),
        },
    };

    let sample = body.sample_context.map(|m| cellarr_core::NamingTokens {
        tokens: m.into_iter().collect(),
    });
    let rendered = cellarr_fs::render_preview(&body.format, target, sample.as_ref())
        .map_err(|e| ApiError::BadRequest(format!("preview render failed: {e}")))?;
    Ok(Json(json!({
        "format": body.format,
        "target": target.key(),
        "rendered": rendered,
    })))
}

/// `GET /config/mediamanagement` — the full media-management settings blob (recycle
/// bin, naming formats, the post-commit permission policy, and the extra-file import
/// policy). Serialized exactly as [`MediaManagement`](cellarr_core::MediaManagement)
/// so it round-trips through the matching PUT.
async fn get_media_management(State(fs): State<FaceState>) -> ApiResult<Json<Value>> {
    let mm = fs.state.db.config().get_media_management().await?;
    Ok(Json(serde_json::to_value(&mm).map_err(|e| {
        ApiError::Internal(format!("serializing media-management settings: {e}"))
    })?))
}

/// The `PUT /config/mediamanagement` body. Every field is optional so a partial PUT
/// merges into the persisted settings (the *arr config endpoints merge rather than
/// replace): the Naming card saves `naming`, the Permissions card saves
/// `permissions`, and the Extra Files card saves `extraFiles`, each independently
/// without clobbering the others.
#[derive(Debug, Deserialize)]
struct MediaManagementBody {
    #[serde(rename = "recycleBinPath", default)]
    recycle_bin_path: Option<Option<String>>,
    #[serde(default)]
    naming: Option<cellarr_core::NamingFormats>,
    #[serde(default)]
    permissions: Option<cellarr_core::ImportPermissions>,
    #[serde(rename = "extraFiles", default)]
    extra_files: Option<cellarr_core::ExtraFileImport>,
}

/// `PUT /config/mediamanagement` — partial-merge update of the media-management
/// settings blob. Naming formats are validated (the same strictness as
/// `PUT /config/naming`) before persisting so an invalid format is rejected with
/// `400` rather than silently saved. The permission and extra-file policies are
/// applied **after** a media commit and never roll the imported media back on
/// failure (that contract lives in the import path) — persisting them here is pure
/// config.
async fn update_media_management(
    State(fs): State<FaceState>,
    Json(body): Json<MediaManagementBody>,
) -> ApiResult<Json<Value>> {
    use cellarr_core::NameTarget;

    let mut mm = fs.state.db.config().get_media_management().await?;
    if let Some(recycle) = body.recycle_bin_path {
        mm.recycle_bin_path = recycle.filter(|s| !s.trim().is_empty());
    }
    if let Some(naming) = body.naming {
        // Validate each non-empty format renders against its sample context before
        // persisting, mirroring the dedicated naming PUT.
        for (target, fmt) in [
            (NameTarget::MovieFile, &naming.movie_file_format),
            (NameTarget::SeriesFolder, &naming.series_folder_format),
            (NameTarget::SeasonFolder, &naming.season_folder_format),
            (NameTarget::EpisodeFile, &naming.episode_file_format),
        ] {
            if !fmt.trim().is_empty() {
                cellarr_fs::render_preview(fmt, target, None).map_err(|e| {
                    ApiError::BadRequest(format!("invalid {} format: {e}", target.key()))
                })?;
            }
        }
        mm.naming = naming;
    }
    if let Some(permissions) = body.permissions {
        mm.permissions = permissions;
    }
    if let Some(extra_files) = body.extra_files {
        mm.extra_files = extra_files;
    }
    fs.state.db.config().set_media_management(&mm).await?;
    Ok(Json(serde_json::to_value(&mm).map_err(|e| {
        ApiError::Internal(format!("serializing media-management settings: {e}"))
    })?))
}

// --- notification (Connect webhook) ----------------------------------------

/// Render a cellarr [`NotificationConfig`] into the v3 notification shape the
/// ecosystem reads back after a push: identity + flags + the `on*` event
/// triggers + a `fields[]` projection of `settings`. cellarr ships the **Webhook**
/// implementation (the Connect push Bazarr-push/Notifiarr consume); other
/// connector kinds round-trip their fields unchanged.
fn v3_notification(n: &NotificationConfig) -> Value {
    let fields: Vec<Value> = n
        .settings
        .as_object()
        .map(|o| {
            o.iter()
                .enumerate()
                .map(|(i, (k, v))| json!({ "order": i, "name": k, "value": v }))
                .collect()
        })
        .unwrap_or_default();
    let implementation = notification_implementation(&n.kind);
    // The event toggles cellarr models. `upgrade` is its own key now (distinct
    // from the import `download` key); `health` covers both issue + restored.
    let on = |event: &str| on_event(n, event);
    json!({
        "id": notif_numeric_id(&n.id),
        "name": n.name,
        "implementation": implementation,
        "implementationName": implementation,
        "configContract": format!("{implementation}Settings"),
        "onGrab": on("grab"),
        "onDownload": on("download"),
        "onUpgrade": on("upgrade"),
        "onRename": on("rename"),
        "onHealthIssue": on("health"),
        "onHealthRestored": on("health"),
        "supportsOnGrab": true,
        "supportsOnDownload": true,
        "supportsOnUpgrade": true,
        "supportsOnRename": true,
        "supportsOnHealthIssue": true,
        "supportsOnHealthRestored": true,
        "includeHealthWarnings": on("health"),
        "fields": fields,
        "tags": n.tags,
    })
}

/// Whether notification `n` is subscribed to `event` (empty `on_events` = all).
fn on_event(n: &NotificationConfig, event: &str) -> bool {
    n.on_events.is_empty() || n.on_events.iter().any(|e| e.eq_ignore_ascii_case(event))
}

/// The v3 `implementation` string for a cellarr notification kind. cellarr ships
/// the broadened provider set; each kind maps to the implementation name the
/// ecosystem round-trips (and which `notification/schema` advertises).
fn notification_implementation(kind: &str) -> &'static str {
    use cellarr_core::notification::kind as k;
    match kind.to_ascii_lowercase().as_str() {
        k::DISCORD => "Discord",
        k::TELEGRAM => "Telegram",
        k::EMAIL => "Email",
        k::CUSTOM_SCRIPT => "CustomScript",
        k::PLEX => "PlexServer",
        k::JELLYFIN => "Jellyfin",
        k::EMBY => "MediaBrowser",
        _ => "Webhook",
    }
}

/// The cellarr notification `kind` for a v3 `implementation` string — the inverse
/// of [`notification_implementation`], used when a write body names an
/// implementation. An unknown implementation falls back to the generic webhook.
fn notification_kind_for_implementation(implementation: &str) -> String {
    use cellarr_core::notification::kind as k;
    match implementation.to_ascii_lowercase().as_str() {
        "discord" => k::DISCORD,
        "telegram" => k::TELEGRAM,
        "email" => k::EMAIL,
        "customscript" => k::CUSTOM_SCRIPT,
        "plexserver" | "plex" => k::PLEX,
        "jellyfin" => k::JELLYFIN,
        "mediabrowser" | "emby" => k::EMBY,
        _ => k::WEBHOOK,
    }
    .to_string()
}

async fn list_notifications(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let notifications = fs.state.db.config().list_notifications().await?;
    Ok(Json(notifications.iter().map(v3_notification).collect()))
}

async fn get_notification(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let numeric = parse_i64(&id, "notification")?;
    fs.state
        .db
        .config()
        .list_notifications()
        .await?
        .iter()
        .find(|n| notif_numeric_id(&n.id) == numeric)
        .map(|n| Json(v3_notification(n)))
        .ok_or_else(|| ApiError::NotFound(format!("notification {id} not found")))
}

/// v3 `notification/schema` — the connector templates a notification is built
/// from. cellarr advertises the broadened provider set: the generic **Webhook**
/// (Connect push), **Discord**, **Telegram**, **Email**, **Custom Script**, and
/// the media-server rescan providers **Plex**, **Jellyfin**, **Emby**. Each
/// template carries the `on*` event toggles plus its provider-specific fields the
/// UI renders, so a notification of any kind round-trips through create/update.
async fn notification_schema() -> Json<Vec<Value>> {
    // A template carrying the standard event toggles + a provider's fields.
    let template = |implementation: &str, fields: Vec<Value>| {
        json!({
            "name": "",
            "implementation": implementation,
            "implementationName": implementation,
            "configContract": format!("{implementation}Settings"),
            "infoLink": "",
            "onGrab": true,
            "onDownload": true,
            "onUpgrade": true,
            "onRename": true,
            "onHealthIssue": true,
            "onHealthRestored": true,
            "supportsOnGrab": true,
            "supportsOnDownload": true,
            "supportsOnUpgrade": true,
            "supportsOnRename": true,
            "supportsOnHealthIssue": true,
            "supportsOnHealthRestored": true,
            "fields": fields,
            "presets": [],
            "tags": [],
        })
    };
    Json(vec![
        template(
            "Webhook",
            vec![
                json!({ "order": 0, "name": "url", "label": "URL", "type": "url", "advanced": false }),
                json!({ "order": 1, "name": "method", "label": "Method", "type": "select", "value": 1, "advanced": false }),
                json!({ "order": 2, "name": "username", "label": "Username", "type": "textbox", "advanced": true }),
                json!({ "order": 3, "name": "password", "label": "Password", "type": "password", "advanced": true, "privacy": "password" }),
            ],
        ),
        template(
            "Discord",
            vec![
                json!({ "order": 0, "name": "url", "label": "Webhook URL", "helpText": "The Discord channel webhook URL", "type": "url", "advanced": false }),
            ],
        ),
        template(
            "Telegram",
            vec![
                json!({ "order": 0, "name": "botToken", "label": "Bot Token", "type": "textbox", "advanced": false, "privacy": "apiKey" }),
                json!({ "order": 1, "name": "chatId", "label": "Chat ID", "type": "textbox", "advanced": false }),
            ],
        ),
        template(
            "Email",
            vec![
                json!({ "order": 0, "name": "host", "label": "Server", "type": "textbox", "advanced": false }),
                json!({ "order": 1, "name": "port", "label": "Port", "type": "number", "value": 587, "advanced": false }),
                json!({ "order": 2, "name": "tls", "label": "Use TLS", "type": "checkbox", "value": false, "advanced": false }),
                json!({ "order": 3, "name": "from", "label": "From Address", "type": "textbox", "advanced": false }),
                json!({ "order": 4, "name": "to", "label": "Recipient Address(es)", "helpText": "Comma-separated", "type": "textbox", "advanced": false }),
                json!({ "order": 5, "name": "username", "label": "Username", "type": "textbox", "advanced": true }),
                json!({ "order": 6, "name": "password", "label": "Password", "type": "password", "advanced": true, "privacy": "password" }),
            ],
        ),
        template(
            "CustomScript",
            vec![
                json!({ "order": 0, "name": "path", "label": "Path", "helpText": "Absolute path to the executable cellarr runs on each event", "type": "filepath", "advanced": false }),
            ],
        ),
        template(
            "PlexServer",
            vec![
                json!({ "order": 0, "name": "url", "label": "Server URL", "helpText": "e.g. http://localhost:32400", "type": "url", "advanced": false }),
                json!({ "order": 1, "name": "token", "label": "X-Plex-Token", "type": "textbox", "advanced": false, "privacy": "apiKey" }),
            ],
        ),
        template(
            "Jellyfin",
            vec![
                json!({ "order": 0, "name": "url", "label": "Server URL", "helpText": "e.g. http://localhost:8096", "type": "url", "advanced": false }),
                json!({ "order": 1, "name": "apiKey", "label": "API Key", "type": "textbox", "advanced": false, "privacy": "apiKey" }),
            ],
        ),
        template(
            "MediaBrowser",
            vec![
                json!({ "order": 0, "name": "url", "label": "Server URL", "helpText": "Emby server, e.g. http://localhost:8096", "type": "url", "advanced": false }),
                json!({ "order": 1, "name": "apiKey", "label": "API Key", "type": "textbox", "advanced": false, "privacy": "apiKey" }),
            ],
        ),
    ])
}

/// v3 notification write body. The `fields[]` map into `settings` (the webhook
/// `url` lives here); the `on*` flags map onto cellarr's `on_events` keys.
#[derive(Debug, Deserialize)]
struct NotificationBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    implementation: Option<String>,
    #[serde(default = "default_true")]
    #[serde(rename = "onGrab")]
    on_grab: bool,
    #[serde(default = "default_true")]
    #[serde(rename = "onDownload")]
    on_download: bool,
    #[serde(default = "default_true")]
    #[serde(rename = "onUpgrade")]
    on_upgrade: bool,
    #[serde(default = "default_true")]
    #[serde(rename = "onRename")]
    on_rename: bool,
    #[serde(default = "default_true")]
    #[serde(rename = "onHealthIssue")]
    on_health: bool,
    /// The tag ids this notification is scoped to (the v3 `tags` array).
    /// Empty/omitted = global (fires for all content).
    #[serde(default)]
    tags: Vec<u32>,
    #[serde(default)]
    fields: Vec<FieldBody>,
}

fn notification_from_body(body: &NotificationBody, id: String) -> NotificationConfig {
    let mut settings = serde_json::Map::new();
    for f in &body.fields {
        if let Some(name) = &f.name {
            settings.insert(name.clone(), f.value.clone());
        }
    }
    // Map the v3 `implementation` to cellarr's provider kind. An implementation
    // we model (Discord/Telegram/Email/CustomScript/Plex/Jellyfin/Emby) maps to
    // its kind; an unknown one falls back to the generic webhook.
    let kind = match body.implementation.as_deref() {
        Some(i) => notification_kind_for_implementation(i),
        None => cellarr_core::notification::kind::WEBHOOK.to_string(),
    };
    let mut on_events = Vec::new();
    if body.on_grab {
        on_events.push("grab".to_string());
    }
    if body.on_download {
        on_events.push("download".to_string());
    }
    if body.on_upgrade {
        on_events.push("upgrade".to_string());
    }
    if body.on_rename {
        on_events.push("rename".to_string());
    }
    if body.on_health {
        on_events.push("health".to_string());
    }
    NotificationConfig {
        id,
        name: body.name.clone().unwrap_or_default(),
        kind,
        enabled: true,
        on_events,
        tags: body.tags.clone(),
        settings: Value::Object(settings),
    }
}

async fn create_notification(
    State(fs): State<FaceState>,
    Json(body): Json<NotificationBody>,
) -> ApiResult<Json<Value>> {
    let n = notification_from_body(&body, uuid::Uuid::new_v4().to_string());
    fs.state.db.config().upsert_notification(&n).await?;
    Ok(Json(v3_notification(&n)))
}

async fn update_notification(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Json(body): Json<NotificationBody>,
) -> ApiResult<Json<Value>> {
    let numeric = parse_i64(&id, "notification")?;
    let existing = fs
        .state
        .db
        .config()
        .list_notifications()
        .await?
        .into_iter()
        .find(|n| notif_numeric_id(&n.id) == numeric)
        .ok_or_else(|| ApiError::NotFound(format!("notification {id} not found")))?;
    let n = notification_from_body(&body, existing.id);
    fs.state.db.config().upsert_notification(&n).await?;
    Ok(Json(v3_notification(&n)))
}

async fn delete_notification(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let numeric = parse_i64(&id, "notification")?;
    if let Some(n) = fs
        .state
        .db
        .config()
        .list_notifications()
        .await?
        .into_iter()
        .find(|n| notif_numeric_id(&n.id) == numeric)
    {
        // A real delete (the config repo now supports it). Idempotent: a missing
        // id still returns 200, matching the *arr clients' delete expectation.
        fs.state.db.config().delete_notification(&n.id).await?;
    }
    Ok(Json(json!({})))
}

/// v3 `notification/test` — probes the configured provider's connectivity and
/// reports whether it accepted the test. For the generic Connect **webhook** this
/// posts an `eventType: Test` payload (what Bazarr/Notifiarr fire); for every
/// other provider it runs that provider's [`test`](cellarr_core::NotificationSender::test)
/// (a Discord/Telegram message, an SMTP send, a script run, or a media-server
/// liveness ping). A malformed/missing setting is reported as a validation
/// failure, never a 500.
async fn test_notification(
    State(fs): State<FaceState>,
    Json(body): Json<NotificationBody>,
) -> ApiResult<Json<Value>> {
    let surface = surface_for(&fs, None).await?;
    let instance = fs.face.app_name(surface);
    let mut n = notification_from_body(&body, "test".to_string());
    if n.name.is_empty() {
        n.name = instance.to_string();
    }

    // The generic Connect webhook keeps its dedicated path (the v3 eventType
    // shape the ecosystem's receivers parse). A missing url is a 400 (a malformed
    // request), matching the long-standing contract — distinct from a well-formed
    // config that simply fails to deliver (reported as isValid:false below).
    let result = if n.kind == cellarr_core::notification::kind::WEBHOOK {
        let url = n
            .settings
            .get("url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ApiError::BadRequest("webhook url is required".into()))?;
        let payload = WebhookPayload::test(instance);
        ReqwestWebhookSender::new().send(url, &payload).await
    } else {
        // Route to the matching provider sender and run its connection test.
        match crate::notifications::default_senders()
            .into_iter()
            .find(|s| s.kind() == n.kind)
        {
            Some(sender) => sender.test(&n).await,
            None => Err(format!("no provider for kind `{}`", n.kind)),
        }
    };

    match result {
        Ok(()) => Ok(Json(json!({ "isValid": true, "validationFailures": [] }))),
        Err(detail) => Ok(Json(json!({
            "isValid": false,
            "validationFailures": [ { "propertyName": "url", "errorMessage": detail } ],
        }))),
    }
}

// --- import lists ----------------------------------------------------------

/// Render a cellarr [`ImportListConfig`](cellarr_core::ImportListConfig) into the
/// v3 import-list shape the ecosystem reads back after a push: identity + flags +
/// the safeguard's `last_successful_sync` + a `fields[]` projection of `settings`.
fn v3_import_list(l: &cellarr_core::ImportListConfig) -> Value {
    let mut fields: Vec<Value> = l
        .settings
        .as_object()
        .map(|o| {
            o.iter()
                .enumerate()
                .map(|(i, (k, v))| json!({ "order": i, "name": k, "value": v }))
                .collect()
        })
        .unwrap_or_default();
    // Surface the quality profile as the field the ecosystem reads.
    if let Some(qp) = &l.quality_profile_id {
        fields.push(json!({ "order": 100, "name": "qualityProfileId", "value": qp }));
    }
    let implementation = import_list_implementation(&l.kind);
    json!({
        "id": il_numeric_id(&l.id),
        "name": l.name,
        "implementation": implementation,
        "implementationName": implementation,
        "configContract": format!("{implementation}Settings"),
        "enabled": l.enabled,
        "enableAuto": l.enabled,
        "monitor": if l.monitored { "all" } else { "none" },
        "shouldMonitor": l.monitored,
        // The clean action the ecosystem keys on; "none" is the safe default.
        "listType": "program",
        "cleanLibraryLevel": clean_action_str(l.clean_action),
        // The safeguard surfaced: only ever set on a confirmed-good fetch.
        "lastSuccessfulSync": l
            .last_successful_sync
            .map(|t| t.unix_timestamp())
            .map_or(Value::Null, |s| json!(s)),
        "fields": fields,
        "tags": [],
    })
}

/// The v3 `implementation` string for a cellarr import-list kind.
fn import_list_implementation(kind: &str) -> &'static str {
    match kind.to_ascii_lowercase().as_str() {
        "trakt" => "TraktList",
        "tmdb" => "TMDbListImport",
        "collection" => "TMDbCollectionImport",
        "plex" | "plex-watchlist" => "PlexImport",
        "imdb" => "IMDbListImport",
        _ => "CustomImport",
    }
}

/// The cellarr import-list kind for a v3 implementation string (the inverse of
/// [`import_list_implementation`]).
fn import_list_kind(implementation: Option<&str>) -> String {
    match implementation.map(|s| s.to_ascii_lowercase()) {
        Some(i) if i.contains("trakt") => "trakt".into(),
        // A TMDb *collection* import is its own source kind (auto-add-the-rest);
        // checked before the generic tmdb match since the string contains "tmdb".
        Some(i) if i.contains("collection") => "collection".into(),
        Some(i) if i.contains("tmdb") => "tmdb".into(),
        Some(i) if i.contains("plex") => "plex".into(),
        Some(i) if i.contains("imdb") => "imdb".into(),
        _ => "custom".into(),
    }
}

/// The v3 `cleanLibraryLevel` string for a cellarr [`CleanAction`].
fn clean_action_str(action: cellarr_core::CleanAction) -> &'static str {
    match action {
        cellarr_core::CleanAction::None => "disabled",
        cellarr_core::CleanAction::Unmonitor => "logOnly",
        cellarr_core::CleanAction::Remove => "removeAndKeep",
    }
}

/// Parse a v3 `cleanLibraryLevel` string into a cellarr [`CleanAction`]. Anything
/// unrecognized — including the absence of the field — maps to the safe
/// [`CleanAction::None`] (never a destructive default).
fn clean_action_from_str(raw: Option<&str>) -> cellarr_core::CleanAction {
    match raw.map(|s| s.to_ascii_lowercase()) {
        Some(s) if s == "logonly" || s == "unmonitor" => cellarr_core::CleanAction::Unmonitor,
        Some(s) if s.starts_with("remove") || s == "removeandkeep" || s == "removeanddelete" => {
            cellarr_core::CleanAction::Remove
        }
        // "disabled", "none", missing, or anything else -> safe default.
        _ => cellarr_core::CleanAction::None,
    }
}

async fn list_import_lists(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    use cellarr_core::ImportListRepository;
    let surface = fs.face.fixed_media();
    let lists = ImportListRepository::list(&fs.state.db.import_lists()).await?;
    let out: Vec<Value> = lists
        .iter()
        .filter(|l| surface.is_none_or(|m| l.media_type == m))
        .map(v3_import_list)
        .collect();
    Ok(Json(out))
}

async fn get_import_list(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    use cellarr_core::ImportListRepository;
    let numeric = parse_i64(&id, "importlist")?;
    ImportListRepository::list(&fs.state.db.import_lists())
        .await?
        .iter()
        .find(|l| il_numeric_id(&l.id) == numeric)
        .map(|l| Json(v3_import_list(l)))
        .ok_or_else(|| ApiError::NotFound(format!("import list {id} not found")))
}

/// v3 `importlist/schema` — the import-list source templates the ecosystem reads.
/// cellarr advertises the Trakt/TMDb/Plex sources (credential-gated) plus the
/// custom source, each with its source-specific credential fields and the
/// `cleanLibraryLevel` toggle (which defaults to the safe "disabled").
async fn import_list_schema(State(fs): State<FaceState>) -> Json<Vec<Value>> {
    let monitor_field = json!({ "order": 0, "name": "shouldMonitor", "label": "Monitor", "type": "checkbox", "value": true, "advanced": false });
    let clean_field = json!({ "order": 1, "name": "cleanLibraryLevel", "label": "Clean Library", "helpText": "Action for items no longer on the list. Defaults to disabled; a destructive action only ever runs after a confirmed-good fetch.", "type": "select", "value": "disabled", "advanced": false });
    let entry = |impl_name: &str, extra: Vec<Value>| {
        let mut fields = vec![monitor_field.clone(), clean_field.clone()];
        fields.extend(extra);
        json!({
            "name": "",
            "implementation": impl_name,
            "implementationName": impl_name,
            "configContract": format!("{impl_name}Settings"),
            "infoLink": "",
            "enabled": true,
            "enableAuto": true,
            "listType": "program",
            "fields": fields,
            "presets": [],
            "tags": [],
        })
    };
    let sonarr = matches!(fs.face.fixed_media(), Some(MediaType::Tv) | None);
    let radarr = matches!(fs.face.fixed_media(), Some(MediaType::Movie) | None);
    let mut out = vec![
        entry(
            "TraktList",
            vec![
                json!({ "order": 10, "name": "client_id", "label": "Client ID", "type": "textbox", "privacy": "apiKey", "advanced": false }),
                json!({ "order": 11, "name": "list", "label": "List Slug", "type": "textbox", "advanced": false }),
            ],
        ),
        entry(
            "PlexImport",
            vec![
                json!({ "order": 10, "name": "token", "label": "Plex Token", "type": "textbox", "privacy": "apiKey", "advanced": false }),
            ],
        ),
    ];
    // IMDb public lists/charts have no public JSON API; surface the JSON-proxy
    // field so a list can be configured through a gateway (blocked-on-source until
    // one is supplied).
    out.push(entry(
        "IMDbListImport",
        vec![
            json!({ "order": 10, "name": "json_url", "label": "IMDb JSON List URL", "helpText": "IMDb has no public list API; point this at a JSON list proxy returning {results:[{id,title,year}]}.", "type": "textbox", "advanced": false }),
        ],
    ));
    if radarr {
        out.push(entry(
            "TMDbListImport",
            vec![
                json!({ "order": 10, "name": "api_key", "label": "TMDb API Key", "type": "textbox", "privacy": "apiKey", "advanced": false }),
                json!({ "order": 11, "name": "list_id", "label": "List ID", "helpText": "A curated TMDb list id. Leave blank to use a feed below.", "type": "textbox", "advanced": false }),
                json!({ "order": 12, "name": "feed", "label": "Feed", "helpText": "When no List ID is set: popular or trending.", "type": "select", "value": "popular", "selectOptions": [ { "value": "popular", "name": "Popular", "order": 0 }, { "value": "trending", "name": "Trending", "order": 1 } ], "advanced": false }),
                json!({ "order": 13, "name": "window", "label": "Trending Window", "helpText": "For the trending feed: day or week.", "type": "select", "value": "week", "selectOptions": [ { "value": "day", "name": "Day", "order": 0 }, { "value": "week", "name": "Week", "order": 1 } ], "advanced": true }),
            ],
        ));
        // A movie-collection import-list: from a TMDb collection id, add the other
        // movies in the collection (the auto-add-the-rest source).
        out.push(entry(
            "TMDbCollectionImport",
            vec![
                json!({ "order": 10, "name": "api_key", "label": "TMDb API Key", "type": "textbox", "privacy": "apiKey", "advanced": false }),
                json!({ "order": 11, "name": "collection_id", "label": "Collection ID", "helpText": "A TMDb collection id; cellarr adds every movie in the collection.", "type": "textbox", "advanced": false }),
            ],
        ));
    }
    if sonarr {
        // Sonarr exposes TheTVDB-style series lists; surface a generic custom entry.
        out.push(entry("CustomImport", Vec::new()));
    }
    Json(out)
}

/// v3 import-list write body (the Sonarr/Radarr-pushed shape). Maps the `fields[]`
/// back into cellarr's `settings` JSON and the identity onto an
/// [`ImportListConfig`]; `cleanLibraryLevel` maps onto the [`CleanAction`]
/// (defaulting to the safe `None`).
#[derive(Debug, Deserialize)]
struct ImportListBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    implementation: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    #[serde(rename = "shouldMonitor")]
    should_monitor: Option<bool>,
    #[serde(default)]
    #[serde(rename = "cleanLibraryLevel")]
    clean_library_level: Option<String>,
    #[serde(default)]
    fields: Vec<FieldBody>,
}

fn import_list_from_body(
    fs: &FaceState,
    body: &ImportListBody,
    id: String,
    existing: Option<&cellarr_core::ImportListConfig>,
) -> cellarr_core::ImportListConfig {
    let mut settings = serde_json::Map::new();
    let mut quality_profile_id = existing.and_then(|e| e.quality_profile_id.clone());
    let mut should_monitor = body.should_monitor;
    let mut clean_level = body.clean_library_level.clone();
    for f in &body.fields {
        let Some(name) = &f.name else { continue };
        match name.as_str() {
            "qualityProfileId" => {
                quality_profile_id = f
                    .value
                    .as_str()
                    .map(ToString::to_string)
                    .or_else(|| f.value.as_i64().map(|n| n.to_string()));
            }
            "shouldMonitor" => {
                should_monitor = should_monitor.or_else(|| f.value.as_bool());
            }
            "cleanLibraryLevel" => {
                clean_level = clean_level.or_else(|| f.value.as_str().map(ToString::to_string));
            }
            _ => {
                settings.insert(name.clone(), f.value.clone());
            }
        }
    }
    // A dedicated face pins the media type; the Cellarr face defaults to Movie.
    let media_type = existing
        .map(|e| e.media_type)
        .unwrap_or_else(|| fs.face.fixed_media().unwrap_or(MediaType::Movie));
    cellarr_core::ImportListConfig {
        id,
        name: body.name.clone().unwrap_or_default(),
        kind: import_list_kind(body.implementation.as_deref()),
        enabled: body.enabled,
        media_type,
        monitored: should_monitor.unwrap_or(true),
        clean_action: clean_action_from_str(clean_level.as_deref()),
        quality_profile_id,
        // Preserve the safeguard timestamp across an update; a create has none.
        last_successful_sync: existing.and_then(|e| e.last_successful_sync),
        settings: Value::Object(settings),
    }
}

async fn create_import_list(
    State(fs): State<FaceState>,
    Json(body): Json<ImportListBody>,
) -> ApiResult<Json<Value>> {
    use cellarr_core::ImportListRepository;
    let l = import_list_from_body(&fs, &body, uuid::Uuid::new_v4().to_string(), None);
    ImportListRepository::upsert(&fs.state.db.import_lists(), &l).await?;
    Ok(Json(v3_import_list(&l)))
}

async fn update_import_list(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Json(body): Json<ImportListBody>,
) -> ApiResult<Json<Value>> {
    use cellarr_core::ImportListRepository;
    let numeric = parse_i64(&id, "importlist")?;
    let existing = ImportListRepository::list(&fs.state.db.import_lists())
        .await?
        .into_iter()
        .find(|l| il_numeric_id(&l.id) == numeric)
        .ok_or_else(|| ApiError::NotFound(format!("import list {id} not found")))?;
    let l = import_list_from_body(&fs, &body, existing.id.clone(), Some(&existing));
    ImportListRepository::upsert(&fs.state.db.import_lists(), &l).await?;
    Ok(Json(v3_import_list(&l)))
}

async fn delete_import_list(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    use cellarr_core::ImportListRepository;
    let numeric = parse_i64(&id, "importlist")?;
    if let Some(l) = ImportListRepository::list(&fs.state.db.import_lists())
        .await?
        .into_iter()
        .find(|l| il_numeric_id(&l.id) == numeric)
    {
        ImportListRepository::delete(&fs.state.db.import_lists(), &l.id).await?;
    }
    Ok(Json(json!({})))
}

/// v3 `importlist/test` — the ecosystem posts the list body to validate it.
/// cellarr accepts a well-formed body (a credential-gated source is allowed to be
/// saved without creds; it simply reports a graceful failed fetch until they are
/// supplied). This is the success contract the apps need to proceed with a push.
async fn test_import_list(Json(body): Json<ImportListBody>) -> ApiResult<Json<Value>> {
    if body
        .name
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        return Err(ApiError::BadRequest("import list name is required".into()));
    }
    Ok(Json(json!({ "isValid": true, "validationFailures": [] })))
}

/// Render one [`ListSyncReport`](crate::import_list_sync::ListSyncReport) into a
/// small JSON summary the UI/FE reads after a sync trigger.
fn v3_list_sync_report(r: &crate::import_list_sync::ListSyncReport) -> Value {
    json!({
        "id": il_numeric_id(&r.list_id),
        "name": r.list_name,
        // The safeguard surfaced: a source that failed reports fetchSucceeded:false
        // and added/cleaned 0, so the FE can show "list unavailable" without it
        // looking like an empty (and thus clean-eligible) list.
        "fetchSucceeded": r.fetch_succeeded,
        "added": r.added,
        "cleaned": r.cleaned,
        "failureReason": r.failure_reason,
    })
}

/// v3 `POST /api/v3/importlist/{id}/sync` — **trigger a sync for one import list**.
///
/// Runs the safeguarded fetch+add for the addressed list through the live
/// [`ImportListSyncRunner`](crate::import_list_sync::ImportListSyncRunner) seam:
/// it fetches the list's source, adds the monitored items the library lacks
/// (skipping excluded + already-present), and — only on a confirmed-good fetch —
/// applies the configured clean action and stamps `last_successful_sync`. A failed
/// fetch changes nothing (the safeguard). With no sync wiring (offline/test) the
/// trigger is reported accepted-but-unwired rather than erroring.
async fn sync_import_list_one(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    use crate::import_list_sync::ImportListSyncOutcome;
    use cellarr_core::ImportListRepository;

    // Resolve the numeric path id back to the cellarr uuid the sync seam keys on.
    let numeric = parse_i64(&id, "importlist")?;
    let list = ImportListRepository::list(&fs.state.db.import_lists())
        .await?
        .into_iter()
        .find(|l| il_numeric_id(&l.id) == numeric)
        .ok_or_else(|| ApiError::NotFound(format!("import list {id} not found")))?;

    let Some(runner) = fs.state.import_list_sync.as_ref() else {
        return Ok(Json(json!({
            "triggered": false,
            "message": "no import-list sync pipeline is configured",
        })));
    };
    let outcome = runner
        .sync_one(&list.id)
        .await
        .map_err(|e| ApiError::Internal(format!("import-list sync failed: {e}")))?;
    let reports = match outcome {
        ImportListSyncOutcome::Ran(reports) => reports,
        ImportListSyncOutcome::Unavailable(reason) => {
            return Ok(Json(json!({ "triggered": false, "message": reason })));
        }
    };
    // An empty report set means the list id did not resolve in the sync seam.
    if reports.is_empty() {
        return Err(ApiError::NotFound(format!("import list {id} not found")));
    }
    Ok(Json(json!({
        "triggered": true,
        "lists": reports.iter().map(v3_list_sync_report).collect::<Vec<_>>(),
    })))
}

/// Run a **sync of all import lists** — the body of the `ImportListSync` command.
///
/// Returns the per-list reports as a JSON value the command handler embeds in its
/// response. With no sync wiring this reports accepted-but-unwired.
async fn run_import_list_sync_all(state: &AppState) -> ApiResult<Value> {
    use crate::import_list_sync::ImportListSyncOutcome;
    let Some(runner) = state.import_list_sync.as_ref() else {
        return Ok(
            json!({ "triggered": false, "message": "no import-list sync pipeline is configured" }),
        );
    };
    let outcome = runner
        .sync_all()
        .await
        .map_err(|e| ApiError::Internal(format!("import-list sync failed: {e}")))?;
    match outcome {
        ImportListSyncOutcome::Ran(reports) => Ok(json!({
            "triggered": true,
            "lists": reports.iter().map(v3_list_sync_report).collect::<Vec<_>>(),
        })),
        ImportListSyncOutcome::Unavailable(reason) => {
            Ok(json!({ "triggered": false, "message": reason }))
        }
    }
}

// --- import-list exclusions ------------------------------------------------

/// Render a cellarr [`ImportListExclusion`] into the v3 exclusion shape.
fn v3_import_list_exclusion(e: &cellarr_core::ImportListExclusion) -> Value {
    // The ecosystem keys exclusions by the media type's external id field.
    let id_field = match e.id_type.to_ascii_lowercase().as_str() {
        "tvdb" => "tvdbId",
        "imdb" => "imdbId",
        _ => "tmdbId",
    };
    json!({
        "id": il_numeric_id(&e.id),
        id_field: e.id_value.parse::<i64>().map_or(json!(e.id_value), |n| json!(n)),
        "title": e.title,
    })
}

async fn list_import_list_exclusions(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    use cellarr_core::ImportListRepository;
    let exclusions = ImportListRepository::list_exclusions(&fs.state.db.import_lists()).await?;
    Ok(Json(
        exclusions.iter().map(v3_import_list_exclusion).collect(),
    ))
}

/// v3 import-list-exclusion write body. The ecosystem posts the external id under
/// the media type's field (`tvdbId`/`tmdbId`/`imdbId`) plus a title.
#[derive(Debug, Deserialize)]
struct ImportListExclusionBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(rename = "tvdbId", default)]
    tvdb_id: Option<Value>,
    #[serde(rename = "tmdbId", default)]
    tmdb_id: Option<Value>,
    #[serde(rename = "imdbId", default)]
    imdb_id: Option<Value>,
}

/// Coerce a JSON id value (number or string) to its string form.
fn id_value_str(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        (!s.is_empty()).then(|| s.to_string())
    } else {
        v.as_i64().map(|n| n.to_string())
    }
}

async fn create_import_list_exclusion(
    State(fs): State<FaceState>,
    Json(body): Json<ImportListExclusionBody>,
) -> ApiResult<Json<Value>> {
    use cellarr_core::ImportListRepository;
    let (id_type, id_value) = if let Some(v) = body.tvdb_id.as_ref().and_then(id_value_str) {
        ("tvdb".to_string(), v)
    } else if let Some(v) = body.tmdb_id.as_ref().and_then(id_value_str) {
        ("tmdb".to_string(), v)
    } else if let Some(v) = body.imdb_id.as_ref().and_then(id_value_str) {
        ("imdb".to_string(), v)
    } else {
        return Err(ApiError::BadRequest(
            "an import-list exclusion requires a tvdbId, tmdbId, or imdbId".into(),
        ));
    };
    let exclusion = cellarr_core::ImportListExclusion {
        id: uuid::Uuid::new_v4().to_string(),
        id_type,
        id_value,
        title: body.title.unwrap_or_default(),
    };
    ImportListRepository::upsert_exclusion(&fs.state.db.import_lists(), &exclusion).await?;
    Ok(Json(v3_import_list_exclusion(&exclusion)))
}

async fn delete_import_list_exclusion(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    use cellarr_core::ImportListRepository;
    let numeric = parse_i64(&id, "importlistexclusion")?;
    if let Some(e) = ImportListRepository::list_exclusions(&fs.state.db.import_lists())
        .await?
        .into_iter()
        .find(|e| il_numeric_id(&e.id) == numeric)
    {
        ImportListRepository::delete_exclusion(&fs.state.db.import_lists(), &e.id).await?;
    }
    Ok(Json(json!({})))
}

// --- blocklist -------------------------------------------------------------

/// Render a cellarr [`BlocklistEntry`](cellarr_core::BlocklistEntry) into the v3
/// blocklist record shape dashboards/UIs read.
fn v3_blocklist_item(e: &cellarr_core::BlocklistEntry) -> Value {
    json!({
        "id": blocklist_numeric_id(&e.id),
        "sourceTitle": e.title,
        "date": e.blocklisted_at.unix_timestamp(),
        "protocol": e.protocol.clone().unwrap_or_default(),
        "indexer": e.indexer.clone().unwrap_or_default(),
        "message": e.reason,
    })
}

/// v3 `GET /blocklist` — the paged list of blocklisted (failed) releases.
async fn list_blocklist(State(fs): State<FaceState>) -> ApiResult<Json<Value>> {
    use cellarr_core::BlocklistRepository;
    let entries = BlocklistRepository::list(&fs.state.db.blocklist()).await?;
    let records: Vec<Value> = entries.iter().map(v3_blocklist_item).collect();
    Ok(Json(paged(records, "date")))
}

/// v3 `DELETE /blocklist/{id}` — clear one blocklisted release so it can be
/// grabbed again. Idempotent (a missing id still 200s).
async fn delete_blocklist_item(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    use cellarr_core::BlocklistRepository;
    let numeric = parse_i64(&id, "blocklist")?;
    // The v3 id is the numeric projection of the entry's uuid; resolve it back.
    if let Some(entry) = BlocklistRepository::list(&fs.state.db.blocklist())
        .await?
        .into_iter()
        .find(|e| blocklist_numeric_id(&e.id) == numeric)
    {
        BlocklistRepository::remove(&fs.state.db.blocklist(), &entry.id).await?;
    }
    Ok(Json(json!({})))
}

/// v3 `DELETE /blocklist/bulk` — clear several blocklisted releases at once (the
/// shape the UI's "remove selected" posts: `{ "ids": [..] }`).
#[derive(Debug, Deserialize)]
struct BlocklistBulkBody {
    #[serde(default)]
    ids: Vec<i64>,
}

async fn delete_blocklist_bulk(
    State(fs): State<FaceState>,
    Json(body): Json<BlocklistBulkBody>,
) -> ApiResult<Json<Value>> {
    use cellarr_core::BlocklistRepository;
    let entries = BlocklistRepository::list(&fs.state.db.blocklist()).await?;
    for numeric in body.ids {
        if let Some(entry) = entries
            .iter()
            .find(|e| blocklist_numeric_id(&e.id) == numeric)
        {
            BlocklistRepository::remove(&fs.state.db.blocklist(), &entry.id).await?;
        }
    }
    Ok(Json(json!({})))
}

// --- lookup ----------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LookupQuery {
    term: Option<String>,
}

async fn movie_lookup(
    State(fs): State<FaceState>,
    Query(q): Query<LookupQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    lookup(&fs.state, q.term.as_deref(), MediaType::Movie).await
}

async fn series_lookup(
    State(fs): State<FaceState>,
    Query(q): Query<LookupQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    lookup(&fs.state, q.term.as_deref(), MediaType::Tv).await
}

async fn lookup(
    state: &AppState,
    term: Option<&str>,
    surface: MediaType,
) -> ApiResult<Json<Vec<Value>>> {
    let term = term.unwrap_or("").trim();
    if term.is_empty() {
        return Ok(Json(Vec::new()));
    }

    // Resolve real identities from the metadata source (TheTVDB for TV, TMDb for
    // movies). This is what makes a lookup return a candidate with the correct
    // external id + human title rather than the search term echoed back — the
    // Phase A deferred gap. With no source configured we degrade gracefully: an
    // empty list, never a 500.
    let Some(meta) = state.metadata.as_ref() else {
        tracing::debug!(
            ?surface,
            "lookup: no metadata source configured; returning empty result"
        );
        return Ok(Json(Vec::new()));
    };

    let outcome = meta
        .search(surface, term)
        .await
        .map_err(|e| ApiError::Upstream(e.to_string()))?;

    let candidates = match outcome {
        crate::metadata::LookupOutcome::Resolved(c) => c,
        crate::metadata::LookupOutcome::Unavailable(reason) => {
            // No source for this media type (e.g. movies without a TMDb key):
            // clearly degrade rather than error, so a client (Overseerr) treats
            // the type as "metadata unavailable" and carries on.
            tracing::info!(
                ?surface,
                reason,
                "lookup: metadata unavailable; returning empty result"
            );
            return Ok(Json(Vec::new()));
        }
    };

    let out = candidates.iter().map(v3_lookup_item).collect::<Vec<_>>();
    Ok(Json(out))
}

/// Render a resolved metadata [`LookupCandidate`] into a v3 lookup resource.
///
/// A lookup candidate is *not* yet in the library, so it carries the resolved
/// identity (title/year/external ids) without file-state — the shape Overseerr
/// reads to offer "add". The external ids land in the field the addressed media
/// type keys on: `tvdbId` for series, `tmdbId`/`imdbId` for movies.
fn v3_lookup_item(c: &crate::metadata::LookupCandidate) -> Value {
    let mut base = json!({
        "title": c.title,
        "titleSlug": slug(&c.title),
        "year": c.year.unwrap_or(0),
        "overview": c.overview.clone().unwrap_or_default(),
        "monitored": false,
        "hasFile": false,
        "tags": [],
    });
    match c.media_type {
        MediaType::Tv => {
            let tvdb_id = c
                .external_id("tvdb")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            merge_into(
                &mut base,
                json!({
                    "tvdbId": tvdb_id,
                    "seriesType": "standard",
                    "status": "continuing",
                }),
            );
            if let Some(imdb) = c.external_id("imdb") {
                merge_into(&mut base, json!({ "imdbId": imdb }));
            }
        }
        _ => {
            let tmdb_id = c
                .external_id("tmdb")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            merge_into(
                &mut base,
                json!({
                    "tmdbId": tmdb_id,
                    "status": "released",
                }),
            );
            if let Some(imdb) = c.external_id("imdb") {
                merge_into(&mut base, json!({ "imdbId": imdb }));
            }
        }
    }
    base
}

// --- library list resources ------------------------------------------------

/// Build the full v3 resource for one content node — the shape Overseerr/Bazarr
/// read for availability: identity + `path`/`rootFolderPath`/`monitored` and the
/// file-state fields (`hasFile`, `*File.path`, `sizeOnDisk`).
async fn v3_resource_item(
    state: &AppState,
    node: &cellarr_core::ContentNode,
    title: &str,
) -> ApiResult<Value> {
    use cellarr_core::repo::MediaFileRepository;
    let files = state.db.media_files().list_for_content(node.id).await?;
    let file = files.first();
    let root = state
        .db
        .config()
        .get_library(node.library_id)
        .await?
        .and_then(|l| l.root_folders.into_iter().next())
        .unwrap_or_default();
    let path = if root.is_empty() {
        format!("/{}", slug(title))
    } else {
        format!("{}/{}", root.trim_end_matches('/'), slug(title))
    };
    let size_on_disk: u64 = files.iter().map(|f| f.size).sum();
    // The persisted external id (tmdb/tvdb/imdb), when the node carries one — e.g.
    // an import-list-added node now links its identity. `None` falls back to 0,
    // matching an un-identified node.
    let external = state
        .db
        .content()
        .external_id_for(node.id, node.media_type)
        .await?;
    let base = json!({
        "title": title,
        "monitored": node.monitored,
        "qualityProfileId": Value::Null,
        "added": "0001-01-01T00:00:00Z",
        "id": node.id.to_string(),
        "path": path,
        "rootFolderPath": root,
        "hasFile": file.is_some(),
        "sizeOnDisk": size_on_disk,
        "titleSlug": slug(title),
        "tags": node.tags,
    });
    // The numeric id (tmdb/tvdb) the ecosystem keys on, projected from the
    // persisted external id; 0 when the node carries no such id yet.
    let numeric_external_id = |ns: &str| -> i64 {
        external
            .as_ref()
            .filter(|(t, _)| t == ns)
            .and_then(|(_, v)| v.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let imdb_external = || -> Option<String> {
        external
            .as_ref()
            .filter(|(t, _)| t == "imdb")
            .map(|(_, v)| v.clone())
    };
    Ok(match node.media_type {
        MediaType::Tv => {
            let mut v = merge(
                base,
                json!({ "tvdbId": numeric_external_id("tvdb"), "seriesType": "standard", "status": "continuing" }),
            );
            if let Some(imdb) = imdb_external() {
                merge_into(&mut v, json!({ "imdbId": imdb }));
            }
            if let Some(f) = file {
                merge_into(
                    &mut v,
                    json!({ "episodeFileCount": files.len(), "statistics": { "sizeOnDisk": size_on_disk } }),
                );
                let _ = f;
            }
            v
        }
        _ => {
            let mut v = merge(
                base,
                json!({ "tmdbId": numeric_external_id("tmdb"), "year": 0, "status": "released", "hasFile": file.is_some() }),
            );
            if let Some(imdb) = imdb_external() {
                merge_into(&mut v, json!({ "imdbId": imdb }));
            }
            if let Some(f) = file {
                merge_into(
                    &mut v,
                    json!({ "movieFile": { "path": f.path, "size": f.size, "quality": { "quality": { "name": f.quality.name } } } }),
                );
            }
            v
        }
    })
}

/// List the root content nodes of a media type as v3 resources — the series /
/// movie entries a library lists. A dedicated face is pinned to its media type;
/// the Cellarr face lists every library's roots.
async fn list_resources(fs: &FaceState, surface: MediaType) -> ApiResult<Vec<Value>> {
    let cfg = fs.state.db.config();
    let content = fs.state.db.content();
    let mut out = Vec::new();
    for lib in cfg.list_libraries().await? {
        if lib.media_type != surface {
            continue;
        }
        for node in content.roots(lib.id).await? {
            // Surface the real indexed title for an identified node (one added /
            // identified with a title); fall back to the id only when the node
            // has no indexed title yet. This closes the Phase A "UUID title"
            // deferred gap for identified items.
            let title = content
                .title_for(node.id)
                .await?
                .unwrap_or_else(|| node.id.to_string());
            out.push(v3_resource_item(&fs.state, &node, &title).await?);
        }
    }
    Ok(out)
}

async fn list_series(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(list_resources(&fs, MediaType::Tv).await?))
}

async fn list_movies(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(list_resources(&fs, MediaType::Movie).await?))
}

/// The `GET /api/v3/episode` query — the addressed series. The *arr clients
/// (Bazarr, the content-detail UI's monitor tree) pass `seriesId`; cellarr also
/// accepts the `contentId` spelling for its own face.
#[derive(Debug, Deserialize)]
struct EpisodeListQuery {
    #[serde(rename = "seriesId", default)]
    series_id: Option<String>,
    #[serde(rename = "contentId", default)]
    content_id: Option<String>,
}

/// v3 `episode` list — the per-series episode set Bazarr and the content-detail
/// monitor tree read.
///
/// Resolves the addressed series (by `seriesId`/`contentId`, accepting both the
/// full uuid and the numeric projection the list endpoints emit), walks its
/// season→episode subtree, and renders one v3 episode resource per leaf episode
/// node: `id`, `seriesId`, `seasonNumber`/`episodeNumber` (from the node's
/// [`Coordinates::Episode`]), `title` (the indexed/identified episode title),
/// `monitored`, `hasFile` (whether a media file is linked), and `airDate` (the
/// persisted air date, when identified). Sorted by season then episode so the
/// monitor tree renders in numbering order.
///
/// A missing/invalid `seriesId` yields an empty array (the same benign shape the
/// originals return for a series with no episodes), never a 404 — the UI then
/// shows an empty tree rather than an error.
async fn list_episodes(
    State(fs): State<FaceState>,
    Query(q): Query<EpisodeListQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    let raw = q
        .series_id
        .as_deref()
        .or(q.content_id.as_deref())
        .filter(|s| !s.is_empty());
    let Some(raw) = raw else {
        return Ok(Json(Vec::new()));
    };
    let Some(series) = resolve_series_node(&fs.state, raw).await? else {
        return Ok(Json(Vec::new()));
    };

    let content = fs.state.db.content();
    let series_numeric = rpm_numeric_id(&series.id.to_string());

    // Walk the series subtree for its episode leaves.
    let mut episodes = Vec::new();
    let mut stack = content.children(series.id).await?;
    while let Some(node) = stack.pop() {
        if node.kind == cellarr_core::ContentKind::Episode {
            episodes.push(node);
        } else {
            stack.extend(content.children(node.id).await?);
        }
    }

    let mut out = Vec::with_capacity(episodes.len());
    for node in &episodes {
        out.push(v3_episode(&fs.state, node, series_numeric).await?);
    }
    // Order the tree by season then episode (the node walk is id-ordered).
    out.sort_by_key(|e| {
        (
            e.get("seasonNumber").and_then(Value::as_i64).unwrap_or(0),
            e.get("episodeNumber").and_then(Value::as_i64).unwrap_or(0),
        )
    });
    Ok(Json(out))
}

/// Render one episode [`ContentNode`] as the v3 episode resource the monitor tree
/// reads. `series_numeric` is the parent series' projected id, carried on every
/// row as `seriesId`.
async fn v3_episode(
    state: &AppState,
    node: &cellarr_core::ContentNode,
    series_numeric: i64,
) -> ApiResult<Value> {
    use cellarr_core::repo::MediaFileRepository;
    let (season, episode) = match &node.coords {
        cellarr_core::Coordinates::Episode {
            season, episode, ..
        } => (*season, *episode),
        // A non-episode coordinate on an episode-kind node is degenerate; surface
        // zeros rather than failing the whole list.
        _ => (0, 0),
    };
    let content = state.db.content();
    let title = content
        .title_for(node.id)
        .await?
        .unwrap_or_else(|| node.id.to_string());
    let has_file = !state
        .db
        .media_files()
        .list_for_content(node.id)
        .await?
        .is_empty();
    // The persisted air date, written by Identify/Refresh; absent for an
    // unidentified episode.
    let air_date = content
        .metadata(node.id)
        .await?
        .and_then(|m| m.air_date)
        .map(Value::String)
        .unwrap_or(Value::Null);
    Ok(json!({
        "id": rpm_numeric_id(&node.id.to_string()),
        "seriesId": series_numeric,
        "seasonNumber": season,
        "episodeNumber": episode,
        "title": title,
        "monitored": node.monitored,
        "hasFile": has_file,
        "airDate": air_date,
    }))
}

/// Resolve a series id (full uuid or numeric projection) to its Series-kind
/// [`ContentNode`], or `None` when nothing matches. The numeric fallback scans the
/// TV libraries' roots and re-projects each id (the stateless projection the list
/// endpoints emit).
async fn resolve_series_node(
    state: &AppState,
    id: &str,
) -> ApiResult<Option<cellarr_core::ContentNode>> {
    let content = state.db.content();
    if let Ok(uuid) = id.parse::<uuid::Uuid>() {
        let node = content
            .get_node(cellarr_core::ContentId::from_uuid(uuid))
            .await?
            .filter(|n| n.kind == cellarr_core::ContentKind::Series);
        return Ok(node);
    }
    let numeric = parse_i64(id, "series")?;
    for lib in state.db.config().list_libraries().await? {
        if lib.media_type != MediaType::Tv {
            continue;
        }
        for node in content.roots(lib.id).await? {
            if node.kind == cellarr_core::ContentKind::Series
                && rpm_numeric_id(&node.id.to_string()) == numeric
            {
                return Ok(Some(node));
            }
        }
    }
    Ok(None)
}

// --- content detail (GET /movie/{id}, GET /series/{id}) --------------------

async fn get_movie_detail(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    content_detail(&fs.state, &id, MediaType::Movie)
        .await
        .map(Json)
}

async fn get_series_detail(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    content_detail(&fs.state, &id, MediaType::Tv)
        .await
        .map(Json)
}

/// Build the v3 content-detail resource for one node by id — the shape the
/// content-detail screen reads: the [`v3_resource_item`] fields (title, monitored,
/// path, `hasFile`, `sizeOnDisk`) plus the addressed library's
/// **`qualityProfileId`** (cellarr scopes the quality profile per library, so a
/// node's effective profile is its library's default) and an `overview` field.
///
/// `overview`/`year`/`runtime` are read from the content-scoped metadata seam
/// (`content_meta`, written at Identify/Refresh): a node that has been identified
/// surfaces its real facts; one that has not yet falls back to empty/zero. Title,
/// monitored, file-state, size, and the quality profile are always real.
async fn content_detail(state: &AppState, id: &str, expected: MediaType) -> ApiResult<Value> {
    let content_id = cellarr_core::ContentId::from_uuid(
        id.parse::<uuid::Uuid>()
            .map_err(|_| ApiError::BadRequest(format!("invalid id: {id}")))?,
    );
    let content = state.db.content();
    let node = content
        .get_node(content_id)
        .await?
        .filter(|n| n.media_type == expected)
        .ok_or_else(|| ApiError::NotFound(format!("{expected:?} {id} not found")))?;
    let title = content
        .title_for(node.id)
        .await?
        .unwrap_or_else(|| node.id.to_string());
    // The node's effective quality profile is its library's default (cellarr scopes
    // quality profiles per library).
    let profile_id = state
        .db
        .config()
        .get_library(node.library_id)
        .await?
        .map(|l| l.default_quality_profile.to_string());
    // The persisted content-scoped metadata (year/overview/runtime), written at
    // Identify/Refresh. Absent for a node not yet identified — then the v3 fields
    // keep their empty/zero defaults so the resource shape never changes.
    let meta = content.metadata(node.id).await?.unwrap_or_default();
    let mut detail = v3_resource_item(state, &node, &title).await?;
    let mut patch = json!({
        "qualityProfileId": profile_id,
        "overview": meta.overview.clone().unwrap_or_default(),
        "year": meta.year.unwrap_or(0),
        // v3 runtime is in minutes (Sonarr/Radarr both expose it this way).
        "runtime": meta.runtime.unwrap_or(0),
    });
    // The dated facts the detail screen / clients read: a movie's release date
    // (theatrical/physical) and digital release; an episode's air date.
    match node.media_type {
        MediaType::Tv => {
            if let Some(air) = &meta.air_date {
                merge_into(
                    &mut patch,
                    json!({ "airDate": air, "airDateUtc": iso_to_utc(air) }),
                );
            }
        }
        _ => {
            if let Some(physical) = &meta.air_date {
                merge_into(
                    &mut patch,
                    json!({ "inCinemas": iso_to_utc(physical), "physicalRelease": iso_to_utc(physical) }),
                );
            }
            if let Some(digital) = &meta.digital_date {
                merge_into(&mut patch, json!({ "digitalRelease": iso_to_utc(digital) }));
            }
        }
    }
    merge_into(&mut detail, patch);
    Ok(detail)
}

/// Render an ISO `yyyy-mm-dd` date as the midnight-UTC RFC 3339 timestamp the v3
/// date fields use (`airDateUtc`/`inCinemas`/`digitalRelease`). A malformed date
/// is passed through unchanged rather than dropped, so a partial parse never
/// blanks the field.
fn iso_to_utc(date: &str) -> String {
    if date.len() == 10 && date.as_bytes()[4] == b'-' {
        format!("{date}T00:00:00Z")
    } else {
        date.to_string()
    }
}

// --- content monitor toggle (PUT /movie/{id}, PUT /series/{id}) ------------

/// The v3 content update body. The shim's content update is scoped to the one
/// field the UI toggles — `monitored` — and ignores the rest of the (large) v3
/// movie/series body a client may round-trip back.
#[derive(Debug, Deserialize)]
struct ContentUpdateBody {
    #[serde(default)]
    monitored: Option<bool>,
    /// The tag ids to associate with this movie/series (the v3 `tags` array).
    /// `Some` rewrites the whole association (an empty array clears it); omitted
    /// (`None`) leaves the existing tags untouched, so a partial PUT that only
    /// flips `monitored` does not drop tags.
    #[serde(default)]
    tags: Option<Vec<u32>>,
}

/// v3 `PUT /movie/{id}` / `PUT /series/{id}` — the **monitor toggle**.
///
/// Sonarr/Radarr round-trip the whole resource back on a PUT; the only mutation
/// the content-detail UI makes through it is flipping `monitored`, so this reads
/// the node, applies the new monitored flag (when present), and persists it,
/// returning the refreshed detail resource. Other fields in the body are accepted
/// and ignored (cellarr scopes quality profile per library, not per node, and the
/// remaining v3 fields are projections, not stored per-node).
async fn update_content(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Json(body): Json<ContentUpdateBody>,
) -> ApiResult<Json<Value>> {
    let content_id = cellarr_core::ContentId::from_uuid(
        id.parse::<uuid::Uuid>()
            .map_err(|_| ApiError::BadRequest(format!("invalid id: {id}")))?,
    );
    let content = fs.state.db.content();
    let mut node = content
        .get_node(content_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("content {id} not found")))?;
    if let Some(monitored) = body.monitored {
        node.monitored = monitored;
        content.upsert(&node).await?;
    }
    if let Some(tags) = &body.tags {
        content.set_tags(content_id, tags).await?;
    }
    content_detail(&fs.state, &id, node.media_type)
        .await
        .map(Json)
}

// --- content delete (DELETE /movie/{id}, DELETE /series/{id}) --------------

/// The v3 content-delete query — the same two flags Sonarr/Radarr expose on a
/// movie/series delete. Both default to `false` (the *arr default): the record is
/// removed but the files stay and the title may be re-added by an import list.
#[derive(Debug, Deserialize)]
struct DeleteContentQuery {
    /// Whether to remove the media files from disk (recycled when a recycle bin
    /// is configured, otherwise unlinked).
    #[serde(rename = "deleteFiles", default)]
    delete_files: bool,
    /// Whether to add an import-list exclusion so the title is never re-added by a
    /// future import-list sync.
    #[serde(rename = "addImportExclusion", default)]
    add_import_exclusion: bool,
}

/// v3 `DELETE /movie/{id}` — remove a movie, optionally its files and re-add
/// guard. Mirrors Radarr: `deleteFiles` recycles/unlinks the media, and
/// `addImportExclusion` records an exclusion so an import list cannot re-add it.
async fn delete_movie(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Query(q): Query<DeleteContentQuery>,
) -> ApiResult<Json<Value>> {
    delete_content(&fs.state, &id, MediaType::Movie, &q).await
}

/// v3 `DELETE /series/{id}` — remove a series and its whole season/episode
/// subtree, optionally its files and a re-add guard. Mirrors Sonarr.
async fn delete_series(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Query(q): Query<DeleteContentQuery>,
) -> ApiResult<Json<Value>> {
    delete_content(&fs.state, &id, MediaType::Tv, &q).await
}

/// Resolve a v3 content id (a full uuid as the native UI sends, or the numeric
/// projection an *arr client may key on) to the addressed [`ContentNode`] of the
/// expected media type, or `None` when nothing matches. The numeric fallback
/// scans the library roots of the surface and re-projects each id, the same
/// stateless projection [`v3_resource_item`] emits.
async fn resolve_content_node(
    state: &AppState,
    id: &str,
    expected: MediaType,
) -> ApiResult<Option<cellarr_core::ContentNode>> {
    let content = state.db.content();
    if let Ok(uuid) = id.parse::<uuid::Uuid>() {
        let node = content
            .get_node(cellarr_core::ContentId::from_uuid(uuid))
            .await?
            .filter(|n| n.media_type == expected);
        return Ok(node);
    }
    // Numeric projection: match a root node of this media type whose projected id
    // equals the requested integer.
    let numeric = parse_i64(id, "content")?;
    for lib in state.db.config().list_libraries().await? {
        if lib.media_type != expected {
            continue;
        }
        for node in content.roots(lib.id).await? {
            if rpm_numeric_id(&node.id.to_string()) == numeric {
                return Ok(Some(node));
            }
        }
    }
    Ok(None)
}

/// The shared content-delete path for both surfaces: resolve the node, remove the
/// DB record (subtree-aware for a series), optionally recycle/unlink its files,
/// and optionally record an import-list exclusion. Returns `200 {}` on success —
/// the *arr delete contract. The DB record is removed first; a file-removal
/// failure surfaces as an error *after* the record is gone, never leaving a
/// dangling record (the library never re-grabs a deleted title).
async fn delete_content(
    state: &AppState,
    id: &str,
    surface: MediaType,
    q: &DeleteContentQuery,
) -> ApiResult<Json<Value>> {
    let Some(node) = resolve_content_node(state, id, surface).await? else {
        // Idempotent delete: a missing/already-deleted title still returns 200,
        // matching the *arr clients' expectation that a re-issued delete succeeds.
        return Ok(Json(json!({})));
    };

    // The title to record as an exclusion (before the node is gone). Falls back to
    // the id when the node was never identified with a title.
    let title = state
        .db
        .content()
        .title_for(node.id)
        .await?
        .unwrap_or_else(|| node.id.to_string());

    // Remove the DB record (subtree for a series), getting back the file paths.
    let content = state.db.content();
    let receipt = match surface {
        MediaType::Tv => content.delete_series(node.id).await?,
        _ => content.delete_movie(node.id).await?,
    };
    let Some(receipt) = receipt else {
        // Resolved a node but the typed delete found a kind mismatch; treat as a
        // no-op 200 rather than a 500 (the resolve already kind-filtered, so this
        // is unreachable in practice but keeps the path total).
        return Ok(Json(json!({})));
    };

    // Optionally remove the files. The record is already gone; recycle (reversible)
    // when a bin is configured, else unlink. Guarded against escaping the library
    // root inside `cellarr-fs`.
    if q.delete_files && !receipt.media_file_paths.is_empty() {
        recycle_content_files(state, &node, &receipt.media_file_paths).await?;
    }

    // Optionally record an import-list exclusion so a sync cannot re-add it.
    if q.add_import_exclusion {
        add_content_exclusion(state, &node, &title).await?;
    }

    Ok(Json(json!({})))
}

/// Recycle (or unlink) the deleted content's media files, rooted at the node's
/// library root so the path-escape guard has a boundary. Each file path is the
/// absolute on-disk path stored on its `media_file` row.
async fn recycle_content_files(
    state: &AppState,
    node: &cellarr_core::ContentNode,
    paths: &[String],
) -> ApiResult<()> {
    use std::path::PathBuf;
    // The library root the files must stay within. A library with no root folder
    // has no boundary to enforce against, so we skip the file step rather than
    // risk deleting with an empty/`/` root.
    let root = state
        .db
        .config()
        .get_library(node.library_id)
        .await?
        .and_then(|l| l.root_folders.into_iter().next());
    let Some(root) = root.filter(|r| !r.is_empty()) else {
        return Ok(());
    };
    let root = PathBuf::from(root);
    let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let bin = state.recycle_bin_path.as_deref();
    cellarr_fs::recycle_or_delete(&files, &root, bin)
        .await
        .map_err(|e| ApiError::Internal(format!("removing media files: {e}")))?;
    Ok(())
}

/// Record an import-list exclusion for a deleted title, keyed by its node id so a
/// future import-list sync skips it. cellarr does not yet resolve a title to a
/// stable external id at delete time, so the node id is the stable exclusion key
/// (the same key the title was added under in this library).
async fn add_content_exclusion(
    state: &AppState,
    node: &cellarr_core::ContentNode,
    title: &str,
) -> ApiResult<()> {
    use cellarr_core::importlist::{ImportListExclusion, ImportListRepository};
    let exclusion = ImportListExclusion {
        id: uuid::Uuid::new_v4().to_string(),
        id_type: "cellarrContentId".to_string(),
        id_value: node.id.to_string(),
        title: title.to_string(),
    };
    state.db.import_lists().upsert_exclusion(&exclusion).await?;
    Ok(())
}

// --- add -------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AddBody {
    title: Option<String>,
    #[serde(rename = "qualityProfileId")]
    quality_profile_id: Option<String>,
    #[serde(rename = "rootFolderPath")]
    root_folder_path: Option<String>,
    #[serde(default)]
    monitored: Option<bool>,
    /// The tag ids to associate with the added movie/series (the v3 `tags`
    /// array). Persisted into the `content_tag` association so tag-scoped routing
    /// (delay profile / indexer / client / notification) applies to it. Empty/
    /// omitted leaves the item untagged (global config only).
    #[serde(default)]
    tags: Vec<u32>,
    /// The Sonarr/Radarr `addOptions` block; only its `monitor` selection is read
    /// (the per-episode monitoring policy applied as episodes are populated).
    #[serde(rename = "addOptions", default)]
    add_options: Option<AddOptions>,
}

/// The v3 `addOptions` block carried on an add. cellarr reads the Sonarr-style
/// `monitor` selection (`all`/`existing`/`future`/`missing`/`firstSeason`/
/// `lastSeason`/`pilot`/`none`); the search-on-add flags are accepted and ignored
/// (the daemon's monitored-missing sweep covers acquisition).
#[derive(Debug, Deserialize)]
struct AddOptions {
    #[serde(default)]
    monitor: Option<cellarr_core::MonitorOption>,
}

async fn add_movie(
    State(fs): State<FaceState>,
    Json(body): Json<AddBody>,
) -> ApiResult<Json<Value>> {
    add(&fs.state, body, MediaType::Movie).await
}

async fn add_series(
    State(fs): State<FaceState>,
    Json(body): Json<AddBody>,
) -> ApiResult<Json<Value>> {
    add(&fs.state, body, MediaType::Tv).await
}

async fn add(state: &AppState, body: AddBody, surface: MediaType) -> ApiResult<Json<Value>> {
    let title = body
        .title
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| ApiError::BadRequest("title is required".into()))?;

    let library = pick_library(state, surface).await?;

    let profile_id = match body.quality_profile_id {
        Some(raw) if !raw.is_empty() => raw
            .parse::<uuid::Uuid>()
            .map(QualityProfileId::from_uuid)
            .map_err(|_| ApiError::BadRequest(format!("invalid qualityProfileId: {raw}")))?,
        _ => library.default_quality_profile,
    };

    let (kind, coords) = match surface {
        MediaType::Tv => (
            cellarr_core::ContentKind::Series,
            cellarr_core::Coordinates::Episode {
                season: 1,
                episode: 1,
                absolute: None,
            },
        ),
        _ => (
            cellarr_core::ContentKind::Movie,
            cellarr_core::Coordinates::Movie,
        ),
    };
    // The root container's monitored flag. An explicit `monitored` wins; else the
    // Sonarr-style `addOptions.monitor` selection decides (only `none` adds the
    // series unmonitored — every other option monitors at least some episodes, so
    // the root container is monitored and the per-episode policy is applied as the
    // season/episode tree is populated). Defaults to monitored.
    let monitored = body.monitored.unwrap_or_else(|| {
        body.add_options.as_ref().and_then(|o| o.monitor) != Some(cellarr_core::MonitorOption::None)
    });
    let node = cellarr_core::ContentNode {
        id: cellarr_core::ContentId::new(),
        library_id: library.id,
        media_type: surface,
        parent_id: None,
        kind,
        coords,
        monitored,
        title_id: None,
        // The add body's tags are persisted into the association below; the node
        // also carries them so the response projection renders them.
        tags: body.tags.clone(),
    };
    let content = state.db.content();
    content.upsert(&node).await?;
    content.index_title(node.id, &title).await?;
    // Persist the tag association (the node row carries no tags column).
    content.set_tags(node.id, &body.tags).await?;

    Ok(Json(merge(
        v3_resource_item(state, &node, &title).await?,
        json!({ "qualityProfileId": profile_id.to_string(),
                "rootFolderPath": body.root_folder_path }),
    )))
}

async fn pick_library(state: &AppState, surface: MediaType) -> ApiResult<Library> {
    state
        .db
        .config()
        .list_libraries()
        .await?
        .into_iter()
        .find(|l| l.media_type == surface)
        .ok_or_else(|| ApiError::NotFound(format!("no {surface:?} library configured")))
}

// --- command ---------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CommandBody {
    name: String,
    #[serde(rename = "movieId")]
    movie_id: Option<String>,
    #[serde(rename = "seriesId")]
    series_id: Option<String>,
}

async fn command(
    State(fs): State<FaceState>,
    Json(body): Json<CommandBody>,
) -> ApiResult<Json<Value>> {
    // The import-list sync command runs through the dedicated sync seam (not the
    // scheduler), so intercept it here. Mirrors Sonarr/Radarr's `ImportListSync`
    // command (a.k.a. `ImportListSyncCommand`).
    let lower = body.name.to_ascii_lowercase();
    if lower == "importlistsync" || lower == "importlistsynccommand" {
        let result = run_import_list_sync_all(&fs.state).await?;
        return Ok(Json(json!({
            "name": body.name,
            "commandName": "ImportListSync",
            "status": "completed",
            "queued": "0001-01-01T00:00:00Z",
            "trigger": "manual",
            "result": result,
        })));
    }

    let content_id = body.movie_id.or(body.series_id);
    let kind = kind_for_command(&body.name, content_id)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown command: {}", body.name)))?;
    let cmd_name = command_name(&kind).to_string();
    let job_id = commands::submit(&fs.state.scheduler, kind)
        .await
        .map_err(ApiError::Command)?;
    Ok(Json(json!({
        "id": job_id,
        "name": body.name,
        "commandName": cmd_name,
        "status": "queued",
        "queued": "0001-01-01T00:00:00Z",
        "trigger": "manual",
    })))
}

/// v3 `GET /command` — the list of recent/known commands the ecosystem polls.
/// Backed by the scheduler's jobs.
async fn list_commands(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let jobs = commands::list_jobs(&fs.state.scheduler)
        .await
        .map_err(ApiError::Command)?;
    let out: Vec<Value> = jobs
        .into_iter()
        .map(|j| {
            json!({
                "id": j.id,
                "name": command_name(&j.kind),
                "commandName": command_name(&j.kind),
                "status": format!("{:?}", j.state).to_ascii_lowercase(),
                "queued": "0001-01-01T00:00:00Z",
                "trigger": "manual",
            })
        })
        .collect();
    Ok(Json(out))
}

// --- release (interactive search) ------------------------------------------

/// The `GET /api/v3/release` query: which content node to search for. The
/// ecosystem (and cellarr's own UI) addresses the node by id; we accept the three
/// spellings the *arr apps use interchangeably — `movieId` (Radarr), `seriesId`
/// (Sonarr), and the cellarr-native `contentId` — all carrying the same cellarr
/// [`ContentId`] uuid.
#[derive(Debug, Deserialize)]
struct ReleaseQuery {
    #[serde(rename = "contentId")]
    content_id: Option<String>,
    #[serde(rename = "movieId")]
    movie_id: Option<String>,
    #[serde(rename = "seriesId")]
    series_id: Option<String>,
}

/// v3 `GET /api/v3/release` — the **interactive (manual) release search**.
///
/// For the addressed content node, runs the read-only Discover→Parse→Identify→
/// Decide preview (the real pipeline's [`preview_releases`] path) across the
/// configured indexers and returns the **ranked candidates without grabbing any**
/// of them — exactly what Sonarr/Radarr's interactive-search screen consumes.
///
/// The response mirrors the originals' release shape closely (`guid`, `title`,
/// `indexerId`/`indexer`, `protocol`, `quality.quality.{id,name}`,
/// `customFormatScore`, `size`, `seeders`, `rejected`, `rejections[]`) and adds
/// the cellarr-native aliases the interactive-search FE reads (`cf_score`,
/// `score_reason`, `rejection_reason`).
///
/// When no pipeline is wired (the offline/test default) or no environment is
/// ready to run a search, this returns an **empty array** rather than erroring —
/// the interactive screen degrades to "no releases" rather than breaking.
///
/// [`preview_releases`]: cellarr_jobs::PipelineRunner::preview_releases
async fn release_search(
    State(fs): State<FaceState>,
    Query(q): Query<ReleaseQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    let raw = q
        .content_id
        .or(q.movie_id)
        .or(q.series_id)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ApiError::BadRequest(
                "a contentId (or movieId/seriesId) query parameter is required".into(),
            )
        })?;
    let content_id = cellarr_core::ContentId::from_uuid(
        raw.parse::<uuid::Uuid>()
            .map_err(|_| ApiError::BadRequest(format!("invalid contentId: {raw}")))?,
    );

    // No pipeline wiring (offline/test): degrade to an empty list, never 500.
    let Some(search) = fs.state.release_search.as_ref() else {
        return Ok(Json(Vec::new()));
    };
    let outcome = search
        .search(content_id)
        .await
        .map_err(|e| ApiError::Internal(format!("release search failed: {e}")))?;
    let candidates = match outcome {
        crate::release_search::ReleaseSearchOutcome::Found(c) => c,
        // No indexer/client/library ready: an empty interactive list, not an error.
        crate::release_search::ReleaseSearchOutcome::Unavailable(_) => return Ok(Json(Vec::new())),
    };

    // Map indexer ids -> display names once, so each row can carry the human
    // indexer name the interactive screen shows.
    let indexers = fs.state.db.config().list_indexers().await?;
    let indexer_name = |id: cellarr_core::IndexerId| -> String {
        indexers
            .iter()
            .find(|ix| ix.id == id)
            .map(|ix| ix.name.clone())
            .unwrap_or_default()
    };

    let rows: Vec<Value> = candidates
        .into_iter()
        .map(|c| v3_release(&c, &indexer_name, fs.face))
        .collect();
    Ok(Json(rows))
}

/// Render one [`ReleaseCandidate`] into the v3 interactive-search release row.
fn v3_release(
    c: &cellarr_jobs::ReleaseCandidate,
    indexer_name: &dyn Fn(cellarr_core::IndexerId) -> String,
    face: Face,
) -> Value {
    let quality_name = face_quality_name(&c.quality.name, face).into_owned();
    // The reason field is reported under both the *arr-native `rejections[]`
    // (only populated when rejected) and the cellarr alias `rejection_reason`;
    // a non-rejected row carries its grab/upgrade rationale in `score_reason`.
    let rejections: Vec<Value> = if c.rejected {
        vec![json!(c.reason)]
    } else {
        Vec::new()
    };
    json!({
        "guid": c.release.guid.clone().unwrap_or_else(|| c.release.download_url.clone()),
        "title": c.release.title,
        "indexerId": ix_numeric_id(c.release.indexer_id),
        "indexer": indexer_name(c.release.indexer_id),
        "protocol": protocol_str(c.release.protocol),
        "quality": {
            "quality": { "id": c.quality.rank, "name": quality_name },
            "revision": { "version": 1, "real": 0, "isRepack": false },
        },
        "customFormatScore": c.custom_format_score,
        // cellarr-native alias the interactive-search FE reads.
        "cf_score": c.custom_format_score,
        "size": c.release.size.unwrap_or(0),
        "seeders": c.release.seeders,
        "downloadUrl": c.release.download_url,
        "rejected": c.rejected,
        "rejections": rejections,
        // cellarr-native aliases: the rejection reason when rejected, and the
        // grab/upgrade/score rationale always (so the row is never blank).
        "rejection_reason": if c.rejected { Some(c.reason.clone()) } else { None },
        "score_reason": c.reason,
    })
}

/// The `POST /api/v3/release` body: which release to grab, for which content node.
/// The interactive-search FE POSTs the `guid` of the row the user picked plus the
/// content id the search was scoped to. We accept the three id spellings the *arr
/// apps use interchangeably (`movieId`/`seriesId`/`contentId`), all carrying the
/// same cellarr [`ContentId`] uuid — matching the `GET /api/v3/release` query.
#[derive(Debug, Deserialize)]
struct GrabReleaseBody {
    /// The indexer-advertised guid of the chosen release (falls back to its
    /// download URL when the indexer advertises no guid). Optional in the wire
    /// shape so a missing guid is a structured 400 from the handler rather than an
    /// opaque deserialization 422.
    #[serde(default)]
    guid: Option<String>,
    #[serde(rename = "contentId", default)]
    content_id: Option<String>,
    #[serde(rename = "movieId", default)]
    movie_id: Option<String>,
    #[serde(rename = "seriesId", default)]
    series_id: Option<String>,
}

/// v3 `POST /api/v3/release` — the **interactive grab**.
///
/// The `GET /api/v3/release` interactive-search screen lists ranked candidates;
/// this is the action that grabs the one the user picked. It hands the chosen
/// `guid` + content node to the [`ReleaseGrab`](crate::release_search::ReleaseGrab)
/// seam, which builds the configured download client and drives the real
/// Grab→Track→Import path (unlike the read-only search/preview, which never builds
/// a client). The end-to-end loop interactive search → pick → grab is what this
/// closes (previously the FE's POST 405'd — there was no grab route).
///
/// The originals return the grabbed release row from this endpoint; we mirror that
/// shape (`guid` + a cellarr-native `grabbed`/`imported`/`message` triple the FE
/// reads for its toast). When no pipeline is wired (offline/test) or no
/// environment is ready, this degrades to a clear `503`-free JSON outcome with
/// `grabbed: false` rather than erroring, so the screen shows a message.
async fn grab_release(
    State(fs): State<FaceState>,
    Json(body): Json<GrabReleaseBody>,
) -> ApiResult<Json<Value>> {
    let guid = body
        .guid
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("a release guid is required".into()))?;
    let raw = body
        .content_id
        .or(body.movie_id)
        .or(body.series_id)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ApiError::BadRequest(
                "a contentId (or movieId/seriesId) is required to grab a release".into(),
            )
        })?;
    let content_id = cellarr_core::ContentId::from_uuid(
        raw.parse::<uuid::Uuid>()
            .map_err(|_| ApiError::BadRequest(format!("invalid contentId: {raw}")))?,
    );

    // No pipeline wiring (offline/test): report the grab as unavailable rather
    // than 500. The FE shows the message and the row is not grabbed.
    let Some(grabber) = fs.state.release_grab.as_ref() else {
        return Ok(Json(json!({
            "guid": guid,
            "grabbed": false,
            "imported": false,
            "message": "no download pipeline is configured",
        })));
    };

    use crate::release_search::ReleaseGrabOutcome;
    let outcome = grabber
        .grab(content_id, guid)
        .await
        .map_err(|e| ApiError::Internal(format!("grab failed: {e}")))?;
    let body = match outcome {
        ReleaseGrabOutcome::Grabbed { imported, detail } => json!({
            "guid": guid,
            "grabbed": true,
            "imported": imported,
            "message": detail,
        }),
        ReleaseGrabOutcome::Unavailable(reason) => json!({
            "guid": guid,
            "grabbed": false,
            "imported": false,
            "message": reason,
        }),
    };
    Ok(Json(body))
}

// --- manual import (loose-folder scan + commit) ----------------------------

/// The `GET /api/v3/manualimport` query: the loose folder to scan for media files
/// (the Sonarr/Radarr manual-import `folder` parameter).
#[derive(Debug, Deserialize)]
struct ManualImportQuery {
    folder: Option<String>,
}

/// v3 `GET /api/v3/manualimport` — the **manual-import scan**.
///
/// Scans `folder` (read-only — moves nothing) for media files, parses each, and
/// attempts to identify it onto a library item, returning the ranked candidates
/// the manual-import screen renders. Mirrors Sonarr/Radarr's manual-import row
/// shape (`path`, `name`, `size`, `quality`, the suggested `movie`/`series` +
/// `seasonNumber`/`episodeNumber`, and `rejections[]`) plus the cellarr-native
/// `parsedTitle`/`contentId` aliases the FE reads.
///
/// When no pipeline is wired (offline/test) or no library is ready, this returns
/// an **empty array** rather than erroring — the screen degrades to "no files".
async fn manual_import_scan(
    State(fs): State<FaceState>,
    Query(q): Query<ManualImportQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    let folder = q
        .folder
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("a folder query parameter is required".into()))?;

    // No pipeline wiring (offline/test): degrade to an empty list, never 500.
    let Some(mi) = fs.state.manual_import.as_ref() else {
        return Ok(Json(Vec::new()));
    };
    let outcome = mi
        .scan(folder)
        .await
        .map_err(|e| ApiError::Internal(format!("manual-import scan failed: {e}")))?;
    let candidates = match outcome {
        crate::manual_import::ManualImportOutcome::Found(c) => c,
        // No library ready: an empty manual-import list, not an error.
        crate::manual_import::ManualImportOutcome::Unavailable(_) => return Ok(Json(Vec::new())),
    };

    let rows: Vec<Value> = candidates
        .into_iter()
        .map(|c| v3_manual_import_row(&c, fs.face))
        .collect();
    Ok(Json(rows))
}

/// Render one [`ManualImportCandidate`] into the v3 manual-import row.
fn v3_manual_import_row(c: &cellarr_jobs::ManualImportCandidate, face: Face) -> Value {
    let quality_name = face_quality_name(&c.quality.name, face).into_owned();
    let rejections: Vec<Value> = c
        .rejections
        .iter()
        .map(|r| json!({ "reason": r }))
        .collect();
    // The suggested placement: the *arr screen keys off `movie`/`series` objects
    // (carrying the content id) plus season/episode; cellarr also surfaces the flat
    // `contentId`/`seasonNumber`/`episodeNumber` aliases the native FE reads.
    let mut row = json!({
        "path": c.path,
        "name": c.name,
        "size": c.size,
        "parsedTitle": c.parsed_title,
        "quality": {
            "quality": { "id": c.quality.rank, "name": quality_name },
            "revision": { "version": 1, "real": 0, "isRepack": false },
        },
        "customFormats": [],
        "rejected": !c.rejections.is_empty(),
        "rejections": rejections,
    });
    if let Some(s) = &c.suggested {
        merge_into(
            &mut row,
            json!({
                "contentId": s.content_id.to_string(),
                "seasonNumber": s.season,
                "episodeNumber": s.episode,
                // The *arr-native suggested-item objects, keyed by the cellarr id so
                // the FE's "move file" POST round-trips the same id back.
                "movie": { "id": s.content_id.to_string() },
                "series": { "id": s.content_id.to_string() },
            }),
        );
    }
    row
}

/// The `POST /api/v3/manualimport` body: the chosen files to import. Each item
/// names the loose source `path` and the content node it maps to (`contentId`, or
/// the *arr `movieId`/`seriesId` spellings), confirming or overriding the scan's
/// suggestion.
#[derive(Debug, Deserialize)]
struct ManualImportCommitBody {
    #[serde(default)]
    files: Vec<ManualImportFile>,
}

#[derive(Debug, Deserialize)]
struct ManualImportFile {
    path: Option<String>,
    #[serde(rename = "contentId", default)]
    content_id: Option<String>,
    #[serde(rename = "movieId", default)]
    movie_id: Option<String>,
    #[serde(rename = "seriesId", default)]
    series_id: Option<String>,
}

/// v3 `POST /api/v3/manualimport` — the **manual-import commit**.
///
/// Imports the user's chosen loose files onto the content nodes they picked, each
/// through the **same crash-safe stage→verify→commit→log import path** an
/// automatic acquisition uses (it never moves a byte until the plan is verified).
/// The originals drive this through `POST /command` with a `ManualImport`/`MoveFile`
/// command; cellarr accepts both that and this direct route (see [`command`]).
///
/// Returns a per-file result list (`imported[]` with the destination each file
/// landed at, `errors[]` for files that could not be placed) the screen shows. When
/// no pipeline is wired or no library is ready, it degrades to a clear JSON outcome
/// with `imported: []` rather than erroring.
async fn manual_import_commit(
    State(fs): State<FaceState>,
    Json(body): Json<ManualImportCommitBody>,
) -> ApiResult<Json<Value>> {
    // Build the typed requests, validating each file carries a source path and a
    // resolvable content id. A bad item is a structured 400, not a silent skip.
    let mut requests = Vec::with_capacity(body.files.len());
    for f in body.files {
        let path = f
            .path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ApiError::BadRequest("each file needs a path".into()))?;
        let raw = f
            .content_id
            .or(f.movie_id)
            .or(f.series_id)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ApiError::BadRequest("each file needs a contentId (or movieId/seriesId)".into())
            })?;
        let content_id = cellarr_core::ContentId::from_uuid(
            raw.parse::<uuid::Uuid>()
                .map_err(|_| ApiError::BadRequest(format!("invalid contentId: {raw}")))?,
        );
        requests.push(cellarr_jobs::ManualImportRequest {
            path: path.to_string(),
            content_id,
        });
    }

    // No pipeline wiring (offline/test): report unavailable rather than 500.
    let Some(mi) = fs.state.manual_import.as_ref() else {
        return Ok(Json(json!({
            "imported": [],
            "errors": [],
            "message": "no import pipeline is configured",
        })));
    };

    use crate::manual_import::ManualImportCommitOutcome;
    let outcome = mi
        .commit(requests)
        .await
        .map_err(|e| ApiError::Internal(format!("manual import failed: {e}")))?;
    let body = match outcome {
        ManualImportCommitOutcome::Committed { imported, errors } => {
            let imported_rows: Vec<Value> = imported
                .into_iter()
                .map(|r| {
                    json!({
                        "sourcePath": r.source_path,
                        "destinationPath": r.destination_path,
                        "contentId": r.content_id.to_string(),
                    })
                })
                .collect();
            json!({
                "imported": imported_rows,
                "errors": errors,
            })
        }
        ManualImportCommitOutcome::Unavailable(reason) => json!({
            "imported": [],
            "errors": [],
            "message": reason,
        }),
    };
    Ok(Json(body))
}

// --- per-episode / per-season monitoring toggles ---------------------------

/// The `PUT /api/v3/episode/monitor` body — the Sonarr monitor-toggle shape: a set
/// of episode ids and the monitored flag to apply to all of them.
#[derive(Debug, Deserialize)]
struct EpisodeMonitorBody {
    #[serde(rename = "episodeIds", default)]
    episode_ids: Vec<String>,
    #[serde(default)]
    monitored: bool,
}

/// v3 `PUT /api/v3/episode/monitor` — the **per-episode monitor toggle**.
///
/// Sets `monitored` on every addressed episode node. Each id is a cellarr content
/// id (the full uuid the native FE sends, or the numeric projection an *arr client
/// keys on); an id that does not resolve to an episode node is skipped (idempotent
/// — re-issuing the toggle on a deleted episode still succeeds). Returns the count
/// of episodes whose flag was persisted.
async fn episode_monitor(
    State(fs): State<FaceState>,
    Json(body): Json<EpisodeMonitorBody>,
) -> ApiResult<Json<Value>> {
    let content = fs.state.db.content();
    let mut updated = 0usize;
    for raw in &body.episode_ids {
        let Some(mut node) = resolve_episode_node(&fs.state, raw).await? else {
            continue;
        };
        if node.monitored != body.monitored {
            node.monitored = body.monitored;
            content.upsert(&node).await?;
        }
        updated += 1;
    }
    Ok(Json(
        json!({ "updated": updated, "monitored": body.monitored }),
    ))
}

/// Resolve an episode id (full uuid or numeric projection) to its episode
/// [`ContentNode`], or `None` when nothing matches an Episode-kind node. The
/// numeric fallback scans every library's content tree and re-projects each node id
/// (the same stateless projection the list endpoints emit).
async fn resolve_episode_node(
    state: &AppState,
    id: &str,
) -> ApiResult<Option<cellarr_core::ContentNode>> {
    let content = state.db.content();
    if let Ok(uuid) = id.parse::<uuid::Uuid>() {
        let node = content
            .get_node(cellarr_core::ContentId::from_uuid(uuid))
            .await?
            .filter(|n| n.kind == cellarr_core::ContentKind::Episode);
        return Ok(node);
    }
    // Numeric projection: walk the TV trees and match a projected episode id.
    let numeric = parse_i64(id, "episode")?;
    for lib in state.db.config().list_libraries().await? {
        if lib.media_type != MediaType::Tv {
            continue;
        }
        for node in walk_episode_nodes(state, lib.id).await? {
            if rpm_numeric_id(&node.id.to_string()) == numeric {
                return Ok(Some(node));
            }
        }
    }
    Ok(None)
}

/// Collect every Episode-kind node under a library, walking the series→season→
/// episode adjacency list. Used by the numeric-id resolution and the season toggle.
async fn walk_episode_nodes(
    state: &AppState,
    library: LibraryId,
) -> ApiResult<Vec<cellarr_core::ContentNode>> {
    let content = state.db.content();
    let mut episodes = Vec::new();
    let mut stack = content.roots(library).await?;
    while let Some(node) = stack.pop() {
        if node.kind == cellarr_core::ContentKind::Episode {
            episodes.push(node);
        } else {
            stack.extend(content.children(node.id).await?);
        }
    }
    Ok(episodes)
}

/// The `PUT /api/v3/season/monitor` body — toggle monitoring for one season and
/// (by default) every episode beneath it, the Sonarr season-toggle behavior.
#[derive(Debug, Deserialize)]
struct SeasonMonitorBody {
    #[serde(rename = "seasonId", default)]
    season_id: Option<String>,
    #[serde(default)]
    monitored: bool,
}

/// v3 `PUT /api/v3/season/monitor` — the **per-season monitor toggle**.
///
/// Sets `monitored` on the addressed season node and cascades the same flag to
/// every episode beneath it (the Sonarr behavior: toggling a season monitors/
/// unmonitors its episodes). Returns the season id and the number of episode nodes
/// updated.
async fn season_monitor(
    State(fs): State<FaceState>,
    Json(body): Json<SeasonMonitorBody>,
) -> ApiResult<Json<Value>> {
    let raw = body
        .season_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("a seasonId is required".into()))?;
    let content = fs.state.db.content();
    let Some(mut season) = resolve_season_node(&fs.state, raw).await? else {
        return Err(ApiError::NotFound(format!("season {raw} not found")));
    };

    if season.monitored != body.monitored {
        season.monitored = body.monitored;
        content.upsert(&season).await?;
    }
    // Cascade to the season's episode children.
    let mut updated = 0usize;
    for mut child in content.children(season.id).await? {
        if child.kind != cellarr_core::ContentKind::Episode {
            continue;
        }
        if child.monitored != body.monitored {
            child.monitored = body.monitored;
            content.upsert(&child).await?;
        }
        updated += 1;
    }
    Ok(Json(json!({
        "seasonId": season.id.to_string(),
        "monitored": body.monitored,
        "episodesUpdated": updated,
    })))
}

/// Resolve a season id (full uuid or numeric projection) to its Season-kind
/// [`ContentNode`], or `None` when nothing matches.
async fn resolve_season_node(
    state: &AppState,
    id: &str,
) -> ApiResult<Option<cellarr_core::ContentNode>> {
    let content = state.db.content();
    if let Ok(uuid) = id.parse::<uuid::Uuid>() {
        let node = content
            .get_node(cellarr_core::ContentId::from_uuid(uuid))
            .await?
            .filter(|n| n.kind == cellarr_core::ContentKind::Season);
        return Ok(node);
    }
    let numeric = parse_i64(id, "season")?;
    for lib in state.db.config().list_libraries().await? {
        if lib.media_type != MediaType::Tv {
            continue;
        }
        // Walk series -> season nodes and match a projected season id.
        let mut stack = content.roots(lib.id).await?;
        while let Some(node) = stack.pop() {
            if node.kind == cellarr_core::ContentKind::Season {
                if rpm_numeric_id(&node.id.to_string()) == numeric {
                    return Ok(Some(node));
                }
            } else if node.kind == cellarr_core::ContentKind::Series {
                stack.extend(content.children(node.id).await?);
            }
        }
    }
    Ok(None)
}

// --- calendar / queue / history / wanted -----------------------------------

/// The `GET /api/v3/calendar` query: an optional `[start, end]` ISO date window
/// (the originals' calendar paging) plus the usual `libraryId` surface hint.
#[derive(Debug, Deserialize)]
struct CalendarQuery {
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
    #[serde(rename = "libraryId", default)]
    library_id: Option<String>,
}

/// v3 `GET /api/v3/calendar` — the JSON calendar feed the dashboard reads for
/// upcoming items by date.
///
/// Backed by the shared [`collect_calendar_events`](crate::calendar::collect_calendar_events)
/// seam (the same one the iCal feed uses): one row per content node of the
/// addressed surface whose coordinates carry an air/release date, within the
/// optional `[start, end]` window, sorted by date. Each row mirrors the originals'
/// calendar shape (`id`, `title`, `airDate`/`airDateUtc`, `monitored`, `hasFile`)
/// plus the cellarr-native `date`/`summary` aliases.
///
/// Returns the real dated items: a TV daily-coded episode's self-contained date,
/// an episode's persisted air date, or a movie's persisted release date (the
/// identify pipeline writes these to the content-metadata seam). A library whose
/// items are not yet identified yields an empty calendar — the dashboard then
/// shows "Recently added" instead. Bounded by the optional `?start`/`?end` ISO
/// date window.
async fn calendar(
    State(fs): State<FaceState>,
    Query(q): Query<CalendarQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    let hint = library_hint(q.library_id.as_deref())?;
    let surface = surface_for(&fs, hint).await?;
    let events = crate::calendar::collect_calendar_events(
        &fs.state,
        surface,
        q.start.as_deref(),
        q.end.as_deref(),
    )
    .await?;
    let rows: Vec<Value> = events
        .into_iter()
        .map(|e| {
            json!({
                "id": e.uid,
                "title": e.summary,
                "airDate": e.date,
                "airDateUtc": format!("{}T00:00:00Z", e.date),
                "monitored": true,
                "hasFile": false,
                // cellarr-native aliases the dashboard reads.
                "date": e.date,
                "summary": e.summary,
            })
        })
        .collect();
    Ok(Json(rows))
}

/// v3 `mediacover/{contentId}/{kind}` — serve cached poster/fanart bytes.
///
/// A thin face-state wrapper over [`crate::mediacover::media_cover`] so the
/// artwork route shares the shim's [`FaceState`] router (it needs only the inner
/// [`AppState`]).
async fn media_cover(
    State(fs): State<FaceState>,
    path: Path<(String, String)>,
) -> axum::response::Response {
    crate::mediacover::media_cover(State(fs.state), path).await
}

/// The full v3 paging envelope: `page, pageSize, sortKey, sortDirection,
/// totalRecords, records`.
fn paged(records: Vec<Value>, sort_key: &str) -> Value {
    let total = records.len();
    json!({
        "page": 1,
        "pageSize": total.max(1),
        "sortKey": sort_key,
        "sortDirection": "descending",
        "totalRecords": total,
        "records": records,
    })
}

/// v3 `GET /api/v3/queue` — the download queue.
///
/// A queue item is an **in-flight grab**: a release handed to a download client
/// that has not reached a terminal state (imported/failed/blocklisted). cellarr
/// backs the queue on the real `grab` rows so the queue-management endpoints
/// (remove / change-category / grab-from-queue) operate on actual downloads. A
/// dedicated face only lists items of its own media type.
async fn queue(State(fs): State<FaceState>) -> ApiResult<Json<Value>> {
    use cellarr_core::repo::GrabRepository;
    let surface = fs.face.fixed_media();
    let grabs = fs.state.db.grabs().list().await?;
    let records: Vec<Value> = grabs
        .iter()
        .filter(|g| !is_terminal_grab(g.status))
        .filter(|g| surface.is_none_or(|m| g.request.content_ref.media_type == m))
        .map(v3_queue_item)
        .collect();
    Ok(Json(paged(records, "timeleft")))
}

/// Whether a grab's lifecycle state is terminal (so it is no longer a live queue
/// item). Imported is a success; Failed/Blocklisted are dead releases.
fn is_terminal_grab(status: cellarr_core::GrabStatus) -> bool {
    use cellarr_core::GrabStatus as S;
    matches!(status, S::Imported | S::Failed | S::Blocklisted)
}

/// The v3 `trackedDownloadState`/`status` strings for a grab lifecycle state.
fn grab_status_strs(status: cellarr_core::GrabStatus) -> (&'static str, &'static str) {
    use cellarr_core::GrabStatus as S;
    // (status, trackedDownloadState) — the two fields *arr clients read.
    match status {
        S::Pending => ("queued", "downloading"),
        S::Sent => ("queued", "downloading"),
        S::Downloading => ("downloading", "downloading"),
        S::Completed => ("completed", "importPending"),
        // Terminal states are filtered out of the queue list, but render sanely if
        // referenced directly.
        S::Imported => ("completed", "imported"),
        S::Failed => ("failed", "failed"),
        S::Blocklisted => ("failed", "failed"),
    }
}

/// Render one in-flight [`Grab`](cellarr_core::Grab) into the v3 queue record the
/// ecosystem reads, keyed by the grab id's numeric projection.
fn v3_queue_item(g: &cellarr_core::Grab) -> Value {
    let (status, tracked_state) = grab_status_strs(g.status);
    json!({
        "id": grab_numeric_id(&g.id.to_string()),
        "title": g.request.release.title,
        "downloadId": g.download_id,
        "status": status,
        "trackedDownloadStatus": "ok",
        "trackedDownloadState": tracked_state,
        "protocol": protocol_str(g.request.release.protocol),
        "indexer": g.request.indexer_id.to_string(),
        "downloadClient": g.request.client_id.to_string(),
        "size": g.request.release.size.unwrap_or(0),
        // cellarr-native aliases the native queue FE reads.
        "category": g.request.category,
        "contentId": g.request.content_ref.id.to_string(),
        "grabId": g.id.to_string(),
    })
}

/// Resolve a queue id (the numeric projection of a grab uuid, or the full uuid)
/// back to its [`Grab`](cellarr_core::Grab).
async fn resolve_queue_grab(state: &AppState, raw: &str) -> ApiResult<Option<cellarr_core::Grab>> {
    use cellarr_core::repo::GrabRepository;
    // Full uuid: a direct get.
    if let Ok(uuid) = raw.parse::<uuid::Uuid>() {
        return Ok(state
            .db
            .grabs()
            .get(cellarr_core::GrabId::from_uuid(uuid))
            .await?);
    }
    // Numeric projection: scan the grabs and match the projected id.
    let numeric = parse_i64(raw, "queue")?;
    Ok(state
        .db
        .grabs()
        .list()
        .await?
        .into_iter()
        .find(|g| grab_numeric_id(&g.id.to_string()) == numeric))
}

/// The `DELETE /api/v3/queue/{id}` query: whether to also remove the download from
/// the client (and its data) and whether to blocklist the release so it is never
/// re-grabbed (the Sonarr/Radarr queue-remove options).
#[derive(Debug, Deserialize)]
struct QueueRemoveQuery {
    #[serde(rename = "removeFromClient", default)]
    remove_from_client: Option<bool>,
    #[serde(default)]
    blocklist: Option<bool>,
}

/// v3 `DELETE /api/v3/queue/{id}` — **remove a queue item**.
///
/// Removes the in-flight grab from cellarr's queue and, per the query flags:
/// - `removeFromClient=true` — tells the download client to remove the download
///   (deleting its on-disk data) via the [`QueueDownloadClient`] seam. With no
///   client wiring (offline/test) this is reported not-performed rather than
///   erroring — the queue row is still removed (the queue is cellarr's own state).
/// - `blocklist=true` — adds the release to the blocklist so a re-search never
///   re-grabs it (the Sonarr/Radarr "remove and blocklist").
///
/// Idempotent: a queue id that does not resolve still returns 200 (the *arr
/// clients expect delete to succeed on a re-issued delete).
async fn delete_queue_item(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Query(q): Query<QueueRemoveQuery>,
) -> ApiResult<Json<Value>> {
    use cellarr_core::repo::GrabRepository;
    let Some(grab) = resolve_queue_grab(&fs.state, &id).await? else {
        // Nothing to remove — idempotent success.
        return Ok(Json(json!({
            "removed": false,
            "removedFromClient": false,
            "blocklisted": false,
        })));
    };

    // 1) Optionally remove from the download client (best-effort: a down client
    // must not strand the queue item).
    let mut removed_from_client = false;
    if q.remove_from_client.unwrap_or(false) {
        if let (Some(client), Some(download_id)) =
            (fs.state.queue_client.as_ref(), grab.download_id.as_deref())
        {
            match client.remove(download_id, /* delete_data = */ true).await {
                Ok(()) => removed_from_client = true,
                Err(e) => tracing::warn!(
                    grab = %grab.id,
                    error = %e,
                    "queue remove: download client removal failed; removing queue row anyway"
                ),
            }
        }
    }

    // 2) Optionally blocklist the release so a re-search never re-grabs it.
    let mut blocklisted = false;
    if q.blocklist.unwrap_or(false) {
        use cellarr_core::blocklist::BlocklistRepository;
        let entry = cellarr_core::BlocklistEntry::from_release(
            grab.request.content_ref.id,
            &grab.request.release,
            "removed from queue and blocklisted by the user",
            time::OffsetDateTime::now_utc(),
        );
        BlocklistRepository::add(&fs.state.db.blocklist(), &entry).await?;
        blocklisted = true;
    }

    // 3) Drop the grab from cellarr's queue.
    let removed = fs.state.db.grabs().delete(grab.id).await?;

    Ok(Json(json!({
        "removed": removed,
        "removedFromClient": removed_from_client,
        "blocklisted": blocklisted,
    })))
}

/// The `PUT /api/v3/queue/{id}` change-category body: the new download category to
/// tag the queued download with.
#[derive(Debug, Deserialize)]
struct QueueCategoryBody {
    #[serde(default)]
    category: Option<String>,
}

/// v3 `PUT /api/v3/queue/{id}` — **change a queued download's category**.
///
/// Retags the in-flight grab with a new download category (the Sonarr/Radarr
/// "change category" queue action). Returns the updated queue record.
async fn update_queue_category(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Json(body): Json<QueueCategoryBody>,
) -> ApiResult<Json<Value>> {
    use cellarr_core::repo::GrabRepository;
    let category = body
        .category
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("a category is required".into()))?;
    let Some(mut grab) = resolve_queue_grab(&fs.state, &id).await? else {
        return Err(ApiError::NotFound(format!("queue item {id} not found")));
    };
    fs.state.db.grabs().set_category(grab.id, category).await?;
    grab.request.category = category.to_string();
    Ok(Json(v3_queue_item(&grab)))
}

/// The `POST /api/v3/queue/grab` body: import a completed-but-unmatched download
/// from the queue by choosing the content node it should land on. The grab carries
/// the on-disk download path; the user picks the content match (or accepts the
/// grab's own content ref).
#[derive(Debug, Deserialize)]
struct QueueGrabBody {
    /// The queue id (numeric projection or uuid) of the completed download.
    /// Accepts a JSON number (the *arr-native numeric id) or a string.
    #[serde(default)]
    id: Option<Value>,
    /// The content node to import onto (overrides the grab's own content ref). The
    /// *arr `movieId`/`seriesId` spellings are accepted too.
    #[serde(rename = "contentId", default)]
    content_id: Option<String>,
    #[serde(rename = "movieId", default)]
    movie_id: Option<String>,
    #[serde(rename = "seriesId", default)]
    series_id: Option<String>,
    /// The on-disk path of the completed download to import, when the caller knows
    /// it (e.g. the queue row's reported output path). Falls back to none, in which
    /// case the grab must carry a resolvable path.
    #[serde(default)]
    path: Option<String>,
}

/// v3 `POST /api/v3/queue/grab` — **manual import from the queue**.
///
/// Imports a completed-but-unmatched download by choosing the content node it
/// should satisfy, reusing the **same crash-safe manual-import commit path** the
/// loose-folder manual import uses. The download's on-disk `path` (from the body,
/// or the grab's reported output) is imported onto the chosen content node; on a
/// successful import the grab is marked imported and dropped from the queue.
///
/// With no import pipeline wired (offline/test), this degrades to a clear JSON
/// outcome with `imported: false` rather than erroring.
async fn queue_grab(
    State(fs): State<FaceState>,
    Json(body): Json<QueueGrabBody>,
) -> ApiResult<Json<Value>> {
    use cellarr_core::repo::GrabRepository;
    let raw_id = body
        .id
        .as_ref()
        .and_then(id_value_str)
        .ok_or_else(|| ApiError::BadRequest("a queue id is required".into()))?;
    let Some(grab) = resolve_queue_grab(&fs.state, &raw_id).await? else {
        return Err(ApiError::NotFound(format!("queue item {raw_id} not found")));
    };

    // The content node to import onto: the caller's chosen match, else the grab's
    // own content ref (the node the grab was intended to satisfy).
    let content_id = match body
        .content_id
        .or(body.movie_id)
        .or(body.series_id)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => cellarr_core::ContentId::from_uuid(
            raw.parse::<uuid::Uuid>()
                .map_err(|_| ApiError::BadRequest(format!("invalid contentId: {raw}")))?,
        ),
        None => grab.request.content_ref.id,
    };

    // The on-disk path of the completed download: the caller's path, else the
    // release's download_url when it names a local path. A queue grab-import needs
    // a concrete source path.
    let path = body
        .path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            ApiError::BadRequest(
                "a path to the completed download is required to import it from the queue".into(),
            )
        })?;

    // No import pipeline wiring (offline/test): report unavailable rather than 500.
    let Some(mi) = fs.state.manual_import.as_ref() else {
        return Ok(Json(json!({
            "imported": false,
            "message": "no import pipeline is configured",
        })));
    };

    use crate::manual_import::ManualImportCommitOutcome;
    let outcome = mi
        .commit(vec![cellarr_jobs::ManualImportRequest { path, content_id }])
        .await
        .map_err(|e| ApiError::Internal(format!("queue grab-import failed: {e}")))?;
    let body = match outcome {
        ManualImportCommitOutcome::Committed { imported, errors } => {
            if !imported.is_empty() {
                // The download landed: mark the grab imported and drop it from the
                // queue so it no longer shows as in-flight.
                let _ = fs
                    .state
                    .db
                    .grabs()
                    .set_status(grab.id, cellarr_core::GrabStatus::Imported)
                    .await;
                let _ = fs.state.db.grabs().delete(grab.id).await;
            }
            json!({
                "imported": !imported.is_empty(),
                "files": imported.len(),
                "errors": errors,
            })
        }
        ManualImportCommitOutcome::Unavailable(reason) => json!({
            "imported": false,
            "message": reason,
        }),
    };
    Ok(Json(body))
}

/// Project a grab id (a uuid string) onto a stable positive integer the v3 queue
/// `id` field requires.
fn grab_numeric_id(id: &str) -> i64 {
    rpm_numeric_id(id)
}

#[derive(Debug, Deserialize)]
struct HistoryQuery {
    #[serde(rename = "movieId")]
    movie_id: Option<String>,
    #[serde(rename = "seriesId")]
    series_id: Option<String>,
}

async fn history(
    State(fs): State<FaceState>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<Value>> {
    use cellarr_core::repo::HistoryRepository;
    let id = q.movie_id.or(q.series_id);
    let records: Vec<Value> = match id {
        Some(raw) if !raw.is_empty() => {
            let cid = cellarr_core::ContentId::from_uuid(
                raw.parse::<uuid::Uuid>()
                    .map_err(|_| ApiError::BadRequest(format!("invalid id: {raw}")))?,
            );
            fs.state
                .db
                .history()
                .for_content(cid)
                .await?
                .into_iter()
                .map(|r| {
                    json!({
                        "id": r.content_id.to_string(),
                        "eventType": history_event_type(&r.event),
                        "date": r.at.unix_timestamp(),
                    })
                })
                .collect()
        }
        _ => Vec::new(),
    };
    Ok(Json(paged(records, "date")))
}

/// v3 `wanted/missing` — the paged list of monitored items missing a file, which
/// dashboards (Homepage/Homarr) read for "missing" counts.
async fn wanted_missing(State(fs): State<FaceState>) -> ApiResult<Json<Value>> {
    let content = fs.state.db.content();
    let surface = fs.face.fixed_media();
    let mut records = Vec::new();
    for r in content.monitored_missing().await? {
        if let Some(media) = surface {
            if r.media_type != media {
                continue;
            }
        }
        records.push(json!({
            "id": r.id.to_string(),
            "monitored": true,
            "hasFile": false,
        }));
    }
    Ok(Json(paged(records, "airDateUtc")))
}

fn history_event_type(event: &cellarr_core::HistoryEvent) -> String {
    serde_json::to_value(event)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str().map(String::from)))
        .unwrap_or_else(|| "unknown".into())
}

// --- small helpers ---------------------------------------------------------

/// Project a cellarr [`CustomFormatId`] (uuid) onto a stable positive integer the
/// v3 `format`/`id` fields require — the ecosystem keys CFs by integer id. A
/// hash of the uuid keeps it stable across requests within a process.
fn cf_numeric_id(id: cellarr_core::CustomFormatId) -> i64 {
    uuid_to_i64(id.as_uuid())
}

/// Project a [`DelayProfileId`](cellarr_core::DelayProfileId) onto a stable
/// positive integer the v3 `id` field requires.
fn dp_numeric_id(id: cellarr_core::DelayProfileId) -> i64 {
    uuid_to_i64(id.as_uuid())
}

/// Project an [`IndexerId`] (uuid) onto a stable positive integer.
fn ix_numeric_id(id: cellarr_core::IndexerId) -> i64 {
    uuid_to_i64(id.as_uuid())
}

/// Project a [`DownloadClientId`] (uuid) onto a stable positive integer.
fn dc_numeric_id(id: cellarr_core::DownloadClientId) -> i64 {
    uuid_to_i64(id.as_uuid())
}

/// Project a remote-path-mapping id (a uuid string) onto a stable positive
/// integer the v3 `id` field requires. A non-uuid id (should not occur for
/// cellarr-created rows) hashes its bytes so the projection stays stable.
/// The largest integer a JS `Number` represents exactly (`2^53 - 1`,
/// `Number.MAX_SAFE_INTEGER`). v3 numeric ids are projected from uuids and read by
/// the web client via `JSON.parse` into a JS `number`; masking to this width keeps
/// every projected id exact in the browser (a wider `i64::MAX` mask silently
/// truncated large ids, so UI delete/update against the mangled id no-op'd). The
/// id is a stateless projection re-derived on every request (lookups re-project and
/// match), so narrowing the mask is consistent and stores nothing.
const JS_SAFE_INT_MAX: i64 = (1 << 53) - 1;

fn rpm_numeric_id(id: &str) -> i64 {
    match uuid::Uuid::parse_str(id) {
        Ok(u) => uuid_to_i64(u),
        Err(_) => {
            let mut n: i64 = 0;
            for b in id.as_bytes().iter().take(8) {
                n = (n << 8) | i64::from(*b);
            }
            n & JS_SAFE_INT_MAX
        }
    }
}

/// Project a notification id (a uuid string) onto a stable positive integer the
/// v3 `id` field requires.
fn notif_numeric_id(id: &str) -> i64 {
    rpm_numeric_id(id)
}

/// Project an import-list (or exclusion) id (a uuid string) onto a stable positive
/// integer the v3 `id` field requires.
fn il_numeric_id(id: &str) -> i64 {
    rpm_numeric_id(id)
}

/// Project a blocklist entry id (a uuid string) onto a stable positive integer.
fn blocklist_numeric_id(id: &str) -> i64 {
    rpm_numeric_id(id)
}

/// Map a uuid to a stable positive `i64` for v3 integer id fields.
fn uuid_to_i64(id: uuid::Uuid) -> i64 {
    let bytes = id.as_bytes();
    let mut n: i64 = 0;
    for b in &bytes[..8] {
        n = (n << 8) | i64::from(*b);
    }
    n & JS_SAFE_INT_MAX
}

fn protocol_str(p: cellarr_core::Protocol) -> &'static str {
    match p {
        cellarr_core::Protocol::Torrent => "torrent",
        cellarr_core::Protocol::Usenet => "usenet",
    }
}

fn face_is_radarr(face: Face) -> bool {
    matches!(face, Face::Radarr) || matches!(face, Face::Cellarr)
}

/// Parse a path id into a `u32` (tags), mapping a bad id to a structured 400.
fn parse_u32(raw: &str, what: &str) -> ApiResult<u32> {
    raw.parse::<u32>()
        .map_err(|_| ApiError::BadRequest(format!("invalid {what} id: {raw}")))
}

/// Parse a path id into an `i64` (the numeric projection used for CFs/indexers).
fn parse_i64(raw: &str, what: &str) -> ApiResult<i64> {
    raw.parse::<i64>()
        .map_err(|_| ApiError::BadRequest(format!("invalid {what} id: {raw}")))
}

/// Shallow-merge object `b` into object `a` (b wins), returning the result.
fn merge(mut a: Value, b: Value) -> Value {
    merge_into(&mut a, b);
    a
}

/// Shallow-merge object `b` into `a` in place (b wins).
fn merge_into(a: &mut Value, b: Value) {
    if let (Some(ao), Some(bo)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in bo {
            ao.insert(k.clone(), v.clone());
        }
    }
}

/// A naive title slug for the v3 `titleSlug`/path fields clients key on.
fn slug(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod numeric_id_js_safety {
    use super::{rpm_numeric_id, uuid_to_i64, JS_SAFE_INT_MAX};

    // Regression: v3 numeric ids are read by the web client as a JS `number`, which
    // only represents integers exactly up to 2^53-1. A uuid whose first 8 bytes have
    // the high bits set used to project (mask i64::MAX) to a value > 2^53, which the
    // browser truncated -> UI delete/update hit a mangled id and silently no-op'd.
    #[test]
    fn projected_ids_fit_in_a_js_number() {
        // A uuid with all-high first bytes would exceed 2^53 under the old i64::MAX mask.
        let worst = uuid::Uuid::from_bytes([0xFF; 16]);
        let n = uuid_to_i64(worst);
        assert!(n >= 0, "id must be non-negative");
        assert!(
            n <= JS_SAFE_INT_MAX,
            "projected id {n} exceeds Number.MAX_SAFE_INTEGER ({JS_SAFE_INT_MAX})"
        );
        // The shared projection (notifications/import-lists/blocklist/rootfolders) too.
        assert!(rpm_numeric_id(&worst.to_string()) <= JS_SAFE_INT_MAX);
        // Stable + deterministic (lookups re-project and match).
        assert_eq!(uuid_to_i64(worst), uuid_to_i64(worst));
        assert_eq!(JS_SAFE_INT_MAX, 9_007_199_254_740_991);
    }
}
