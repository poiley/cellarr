//! The native cellarr REST API under `/api/v1`.
//!
//! Clean, versioned CRUD + commands for the cellarr web UI and new integrations
//! (docs/09-api.md). Reads go through `cellarr-db` repositories; commands go
//! through the `cellarr-jobs` scheduler. Mutating routes sit behind the API-key
//! middleware; reads stay open so the UI works on a zero-config first run.
//!
//! Errors are the structured [`ApiError`] bodies (`code` + `message`), never
//! bare statuses, so clients branch on `code`.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{middleware, Json, Router};
use cellarr_core::repo::{ContentRepository, ProfileRepository};
use cellarr_core::{
    ContentId, DownloadClientConfig, IndexerConfig, Library, LibraryId, MediaType, QualityProfile,
    QualityProfileId,
};
use serde::{Deserialize, Serialize};

use crate::auth::require_api_key;
use crate::commands::{self, command_name, kind_for_command, list_jobs};
use crate::error::{ApiError, ApiResult};
use crate::events::DomainEvent;
use crate::state::AppState;
use crate::stream;

/// Build the `/api/v1` router. Read routes are open; mutating routes are wrapped
/// in the API-key middleware. The two sub-routers are merged so the auth layer
/// applies to exactly the mutating set.
pub fn router(state: AppState) -> Router {
    let reads = Router::new()
        .route("/system/status", get(system_status))
        .route("/libraries", get(list_libraries))
        .route("/libraries/{id}", get(get_library))
        .route("/libraries/{id}/content", get(list_content))
        .route("/content/{id}", get(get_content))
        .route("/content/{id}/files", get(list_content_files))
        .route("/content/{id}/history", get(content_history))
        .route("/indexers", get(list_indexers))
        .route("/downloadclients", get(list_download_clients))
        .route("/qualityprofiles", get(get_quality_profiles))
        .route("/qualityprofiles/{id}", get(get_quality_profile))
        .route("/customformats", get(list_custom_formats))
        .route("/queue", get(get_queue))
        .route("/history", get(get_history))
        .route("/decisionlog/{run_id}", get(get_decision_log))
        .route("/commands", get(get_commands))
        .route("/stream", get(stream::sse))
        .route("/openapi.json", get(openapi))
        .with_state(state.clone());

    let writes = Router::new()
        .route("/libraries", post(create_library))
        .route("/indexers", post(create_indexer))
        .route("/downloadclients", post(create_download_client))
        .route("/commands", post(run_command))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_key,
        ))
        .with_state(state.clone());

    // The web-UI auth-config admin endpoints (`/api/v1/auth/config`,
    // `/api/v1/auth/credential`). These are governed by the web-UI gate (the outer
    // `webauth::gate` layer), NOT the apikey middleware — they manage the UI's own
    // login, so requiring the *arr apikey would be the wrong gate. Before a
    // credential exists the install is open, so first-time setup is reachable; once
    // a credential is set the gate makes them admin-only.
    let auth = crate::webauth::config_routes(state);

    reads.merge(writes).merge(auth)
}

// --- system/status ---------------------------------------------------------

/// System health snapshot. Lightweight and unauthenticated so the UI and
/// monitoring can read it on first run.
#[derive(Debug, Serialize)]
struct SystemStatus {
    app_name: &'static str,
    version: &'static str,
    /// Whether API-key auth is enforced on mutating endpoints.
    auth_enabled: bool,
    library_count: usize,
    indexer_count: usize,
    download_client_count: usize,
    /// Loud filesystem health warnings — currently the cross-filesystem
    /// (silent-copy-fallback) case where a configured downloads dir and a
    /// library root are on different filesystems, so imports cannot hardlink.
    /// Empty when the layout is healthy.
    filesystem_warnings: Vec<HealthWarning>,
}

/// A native-shaped health warning record (the v3 shim renders the same data in
/// its `{ source, type, message, wikiUrl }` shape).
#[derive(Debug, Serialize)]
struct HealthWarning {
    source: &'static str,
    message: String,
}

async fn system_status(State(state): State<AppState>) -> ApiResult<Json<SystemStatus>> {
    let cfg = state.db.config();
    let libraries = cfg.list_libraries().await?;
    let indexers = cfg.list_indexers().await?;
    let clients = cfg.list_download_clients().await?;
    let filesystem_warnings = crate::fs_health::filesystem_warnings(&state.db)
        .await?
        .into_iter()
        .map(|w| HealthWarning {
            source: w.source(),
            message: w.message(),
        })
        .collect();
    Ok(Json(SystemStatus {
        app_name: "cellarr",
        version: env!("CARGO_PKG_VERSION"),
        auth_enabled: !state.auth.accepts(None),
        library_count: libraries.len(),
        indexer_count: indexers.len(),
        download_client_count: clients.len(),
        filesystem_warnings,
    }))
}

// --- libraries -------------------------------------------------------------

async fn list_libraries(State(state): State<AppState>) -> ApiResult<Json<Vec<Library>>> {
    Ok(Json(state.db.config().list_libraries().await?))
}

/// Parse a UUID path segment into a typed id, mapping a bad id to a structured
/// `bad_request` rather than a 404, so a client can tell "malformed" from
/// "missing".
fn parse_uuid(raw: &str, what: &str) -> ApiResult<uuid::Uuid> {
    raw.parse::<uuid::Uuid>()
        .map_err(|_| ApiError::BadRequest(format!("invalid {what} id: {raw}")))
}

async fn get_library(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Library>> {
    let lib_id = LibraryId::from_uuid(parse_uuid(&id, "library")?);
    state
        .db
        .config()
        .get_library(lib_id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("library {id} not found")))
}

/// Body for creating a library.
#[derive(Debug, Deserialize)]
struct CreateLibrary {
    media_type: MediaType,
    name: String,
    #[serde(default)]
    root_folders: Vec<String>,
    default_quality_profile: QualityProfileId,
}

async fn create_library(
    State(state): State<AppState>,
    Json(body): Json<CreateLibrary>,
) -> ApiResult<Json<Library>> {
    let library = Library {
        id: LibraryId::new(),
        media_type: body.media_type,
        name: body.name,
        root_folders: body.root_folders,
        default_quality_profile: body.default_quality_profile,
    };
    state.db.config().upsert_library(&library).await?;
    Ok(Json(library))
}

// --- content ---------------------------------------------------------------

/// A content node listing is rooted at a library: we return the library's root
/// nodes (movies/series/artists/authors). Deeper levels are walked via
/// `/content/{id}` children links the UI follows on demand. The DB layer exposes
/// `children(parent)` but not "roots of a library", so roots are discovered from
/// the monitored-missing set scoped to the library — a documented limitation
/// reported as a core gap; it returns the nodes the pipeline currently tracks.
async fn list_content(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<cellarr_core::ContentRef>>> {
    let lib_id = LibraryId::from_uuid(parse_uuid(&id, "library")?);
    // Confirm the library exists so a bad id is a 404, not an empty list.
    if state.db.config().get_library(lib_id).await?.is_none() {
        return Err(ApiError::NotFound(format!("library {id} not found")));
    }
    let content = state.db.content();
    let refs: Vec<_> = content
        .monitored_missing()
        .await?
        .into_iter()
        .filter(|r| r.library_id == lib_id)
        .collect();
    Ok(Json(refs))
}

async fn get_content(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<cellarr_core::ContentNode>> {
    let content_id = ContentId::from_uuid(parse_uuid(&id, "content")?);
    state
        .db
        .content()
        .get_node(content_id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("content {id} not found")))
}

async fn list_content_files(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<cellarr_core::MediaFile>>> {
    use cellarr_core::repo::MediaFileRepository;
    let content_id = ContentId::from_uuid(parse_uuid(&id, "content")?);
    Ok(Json(
        state.db.media_files().list_for_content(content_id).await?,
    ))
}

async fn content_history(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<cellarr_core::HistoryRecord>>> {
    use cellarr_core::repo::HistoryRepository;
    let content_id = ContentId::from_uuid(parse_uuid(&id, "content")?);
    Ok(Json(state.db.history().for_content(content_id).await?))
}

// --- indexers --------------------------------------------------------------

async fn list_indexers(State(state): State<AppState>) -> ApiResult<Json<Vec<IndexerConfig>>> {
    Ok(Json(state.db.config().list_indexers().await?))
}

async fn create_indexer(
    State(state): State<AppState>,
    Json(indexer): Json<IndexerConfig>,
) -> ApiResult<Json<IndexerConfig>> {
    state.db.config().upsert_indexer(&indexer).await?;
    Ok(Json(indexer))
}

// --- download clients ------------------------------------------------------

async fn list_download_clients(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<DownloadClientConfig>>> {
    Ok(Json(state.db.config().list_download_clients().await?))
}

async fn create_download_client(
    State(state): State<AppState>,
    Json(client): Json<DownloadClientConfig>,
) -> ApiResult<Json<DownloadClientConfig>> {
    state.db.config().upsert_download_client(&client).await?;
    Ok(Json(client))
}

// --- quality profiles / custom formats -------------------------------------

/// Query for one or more profile ids. The DB layer has no `list_profiles`, so
/// the collection read resolves the ids the caller asks for; an empty query
/// returns an empty list rather than guessing. Reported as a core gap.
#[derive(Debug, Deserialize)]
struct ProfileQuery {
    #[serde(default)]
    ids: Option<String>,
}

async fn get_quality_profiles(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<ProfileQuery>,
) -> ApiResult<Json<Vec<QualityProfile>>> {
    let repo = state.db.profiles();
    let mut out = Vec::new();
    if let Some(ids) = q.ids {
        for raw in ids.split(',').filter(|s| !s.is_empty()) {
            let pid = QualityProfileId::from_uuid(parse_uuid(raw, "quality profile")?);
            if let Some(p) = repo.get_profile(pid).await? {
                out.push(p);
            }
        }
    }
    Ok(Json(out))
}

async fn get_quality_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<QualityProfile>> {
    let pid = QualityProfileId::from_uuid(parse_uuid(&id, "quality profile")?);
    state
        .db
        .profiles()
        .get_profile(pid)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("quality profile {id} not found")))
}

async fn list_custom_formats(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<cellarr_core::CustomFormat>>> {
    Ok(Json(state.db.profiles().custom_formats().await?))
}

// --- queue -----------------------------------------------------------------

/// A queue entry as the UI/ecosystem expect it. Backed by the scheduler's
/// in-flight/pending jobs, which is what "the queue" means in cellarr's command
/// model (downloads-in-progress will join this view once the download-client
/// poller publishes through the same bus).
#[derive(Debug, Serialize)]
struct QueueEntry {
    id: String,
    command: String,
    state: String,
    attempts: u32,
}

async fn get_queue(State(state): State<AppState>) -> ApiResult<Json<Vec<QueueEntry>>> {
    let jobs = list_jobs(&state.scheduler)
        .await
        .map_err(ApiError::Command)?;
    let entries = jobs
        .into_iter()
        .map(|j| QueueEntry {
            id: j.id,
            command: command_name(&j.kind).to_string(),
            state: format!("{:?}", j.state).to_ascii_lowercase(),
            attempts: j.attempts,
        })
        .collect();
    Ok(Json(entries))
}

// --- history ---------------------------------------------------------------

/// The global history view requires a content id; the DB layer indexes history
/// per content node, not globally. A `content` query selects the node. Reported
/// as a core gap (no global history scan).
#[derive(Debug, Deserialize)]
struct HistoryQuery {
    content: Option<String>,
}

async fn get_history(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<HistoryQuery>,
) -> ApiResult<Json<Vec<cellarr_core::HistoryRecord>>> {
    use cellarr_core::repo::HistoryRepository;
    let Some(content) = q.content else {
        return Ok(Json(Vec::new()));
    };
    let content_id = ContentId::from_uuid(parse_uuid(&content, "content")?);
    Ok(Json(state.db.history().for_content(content_id).await?))
}

// --- decision log ----------------------------------------------------------

async fn get_decision_log(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> ApiResult<Json<Vec<cellarr_core::DecisionLogRecord>>> {
    let run = cellarr_core::PipelineRunId::from_uuid(parse_uuid(&run_id, "run")?);
    Ok(Json(state.db.decision_log().for_run(run).await?))
}

// --- commands --------------------------------------------------------------

/// The set of commands the API exposes, for discovery by the UI.
#[derive(Debug, Serialize)]
struct CommandInfo {
    name: &'static str,
    description: &'static str,
}

async fn get_commands() -> Json<Vec<CommandInfo>> {
    Json(vec![
        CommandInfo {
            name: "RssSync",
            description: "Sync the latest releases from configured indexers.",
        },
        CommandInfo {
            name: "MissingItemSearch",
            description: "Search for monitored items missing an acceptable file.",
        },
        CommandInfo {
            name: "ManualSearch",
            description: "Search for one specific content node (requires contentId).",
        },
        CommandInfo {
            name: "RefreshMetadata",
            description: "Refresh metadata for known content.",
        },
        CommandInfo {
            name: "DiskSpaceCheck",
            description: "Run a disk-space / health check.",
        },
    ])
}

/// Body for triggering a command.
#[derive(Debug, Deserialize)]
struct RunCommand {
    name: String,
    /// Required for `ManualSearch`: the content node to search for.
    #[serde(default)]
    content_id: Option<String>,
}

/// Accepted-command response.
#[derive(Debug, Serialize)]
struct CommandAccepted {
    job_id: String,
    name: String,
    status: &'static str,
}

async fn run_command(
    State(state): State<AppState>,
    Json(body): Json<RunCommand>,
) -> ApiResult<Json<CommandAccepted>> {
    let kind = kind_for_command(&body.name, body.content_id.clone())
        .ok_or_else(|| ApiError::BadRequest(format!("unknown command: {}", body.name)))?;
    let name = command_name(&kind).to_string();
    let job_id = commands::submit(&state.scheduler, kind)
        .await
        .map_err(ApiError::Command)?;
    // The fired command already published a CommandQueued event with an empty id
    // from the handler; publish a second carrying the real job id so listeners
    // can correlate. Both are real transitions (submitted + run).
    state.events.publish(DomainEvent::CommandQueued {
        job_id: job_id.clone(),
        name: name.clone(),
    });
    Ok(Json(CommandAccepted {
        job_id,
        name,
        status: "queued",
    }))
}

// --- openapi ---------------------------------------------------------------

async fn openapi() -> Json<serde_json::Value> {
    Json(crate::openapi::spec())
}
