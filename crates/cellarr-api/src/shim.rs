//! The `/api/v3` Radarr/Sonarr compatibility shim.
//!
//! A separate router presenting the originals' v3 request/response shapes so the
//! existing ecosystem (Overseerr/Jellyseerr, Notifiarr, …) works unmodified —
//! a non-negotiable external contract (docs/09-api.md). Because cellarr is
//! unified, the shim answers as **Radarr** for movie libraries and **Sonarr**
//! for TV libraries: the response surface is chosen by the addressed library's
//! [`MediaType`].
//!
//! The shapes here are reconstructed clean-room from the *documented* v3 field
//! names the ecosystem clients read; they are intentionally the minimal subset
//! those tools actually consume. They are pinned by contract tests against
//! synthetic recorded pairs.

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{middleware, Json, Router};
use cellarr_core::repo::ProfileRepository;
use cellarr_core::{Library, LibraryId, MediaType, QualityProfileId};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::require_api_key;
use crate::commands::{self, command_name, kind_for_command};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Build the `/api/v3` router. Mutating endpoints (`add`, `command`) require the
/// API key — the ecosystem sends it as `?apikey=` or `X-Api-Key`, which our auth
/// middleware accepts. Reads stay open, matching the originals' behavior for
/// status/lookup under a configured key (clients always send the key anyway).
pub fn router(state: AppState) -> Router {
    let reads = Router::new()
        .route("/system/status", get(system_status))
        .route("/qualityprofile", get(quality_profiles))
        .route("/movie/lookup", get(movie_lookup))
        .route("/series/lookup", get(series_lookup))
        .route("/calendar", get(calendar))
        .route("/queue", get(queue))
        .route("/history", get(history))
        .with_state(state.clone());

    let writes = Router::new()
        .route("/movie", post(add_movie))
        .route("/series", post(add_series))
        .route("/command", post(command))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_key,
        ))
        .with_state(state);

    reads.merge(writes)
}

// --- helpers ---------------------------------------------------------------

/// Resolve which app surface to present. A `libraryId`/`movieId`/`seriesId`
/// query or the presence of TV-shaped libraries decides; absent any hint we look
/// at the configured libraries and prefer Radarr (movies) since that is the most
/// common Overseerr target. The chosen [`MediaType`] is what the shim mimics.
async fn surface_for(state: &AppState, hint: Option<LibraryId>) -> ApiResult<MediaType> {
    let libs = state.db.config().list_libraries().await?;
    if let Some(id) = hint {
        if let Some(lib) = libs.iter().find(|l| l.id == id) {
            return Ok(lib.media_type);
        }
    }
    // No explicit hint: pick by what exists, defaulting to Movie.
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

// --- system/status ---------------------------------------------------------

/// The v3 `system/status` payload, with the `appName` set per the surface so a
/// probing client identifies the right app (Radarr vs Sonarr).
#[derive(Debug, Deserialize)]
struct StatusQuery {
    #[serde(rename = "libraryId")]
    library_id: Option<String>,
}

async fn system_status(
    State(state): State<AppState>,
    Query(q): Query<StatusQuery>,
) -> ApiResult<Json<Value>> {
    let hint = library_hint(q.library_id.as_deref())?;
    let surface = surface_for(&state, hint).await?;
    let app_name = match surface {
        MediaType::Tv => "Sonarr",
        // Radarr is the surface for movies; music/books also present as Radarr-
        // shaped status since the ecosystem clients only know the two.
        _ => "Radarr",
    };
    Ok(Json(json!({
        "appName": app_name,
        "instanceName": app_name,
        "version": "3.0.0",
        "buildTime": "2024-01-01T00:00:00Z",
        "isProduction": true,
        "authentication": if state.auth.accepts(None) { "none" } else { "apikey" },
        "startupPath": "/",
        "appData": "/config",
        "runtimeName": "cellarr",
        "runtimeVersion": env!("CARGO_PKG_VERSION"),
    })))
}

// --- qualityprofile --------------------------------------------------------

/// v3 quality profiles. We surface cellarr's profiles in the v3 list shape the
/// ecosystem reads (`id`, `name`, `items[]`, `cutoff`). Profiles are resolved
/// from the libraries' default profile ids (the DB layer has no list-all).
async fn quality_profiles(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    let libs = state.db.config().list_libraries().await?;
    let repo = state.db.profiles();
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for lib in libs {
        let pid = lib.default_quality_profile;
        if !seen.insert(pid.as_uuid()) {
            continue;
        }
        if let Some(profile) = repo.get_profile(pid).await? {
            out.push(v3_quality_profile(&profile));
        }
    }
    Ok(Json(out))
}

/// Render a cellarr [`QualityProfile`] into the v3 quality-profile shape.
fn v3_quality_profile(p: &cellarr_core::QualityProfile) -> Value {
    let items: Vec<Value> = p
        .allowed_qualities
        .iter()
        .map(|rank| {
            json!({
                "quality": { "id": rank, "name": format!("rank-{rank}") },
                "allowed": true,
            })
        })
        .collect();
    json!({
        "id": p.id.to_string(),
        "name": p.name,
        "upgradeAllowed": p.upgrades_allowed,
        "cutoff": p.cutoff_quality,
        "minFormatScore": p.min_custom_format_score,
        "cutoffFormatScore": p.upgrade_until_custom_format_score,
        "items": items,
    })
}

// --- lookup ----------------------------------------------------------------

/// v3 lookup takes `?term=`. cellarr resolves it against the FTS title index and
/// returns matched content nodes in the per-app lookup shape (movie or series).
#[derive(Debug, Deserialize)]
struct LookupQuery {
    term: Option<String>,
}

async fn movie_lookup(
    State(state): State<AppState>,
    Query(q): Query<LookupQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    lookup(&state, q.term.as_deref(), MediaType::Movie).await
}

async fn series_lookup(
    State(state): State<AppState>,
    Query(q): Query<LookupQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    lookup(&state, q.term.as_deref(), MediaType::Tv).await
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
    let content = state.db.content();
    let ids = content.search(term).await?;
    let mut out = Vec::new();
    for id in ids {
        if let Some(node) = content.get_node(id).await? {
            // A movie lookup only returns movie-shaped results, series lookup
            // only series — the ecosystem calls the right endpoint per type.
            if node.media_type == surface {
                out.push(v3_lookup_item(&node, term));
            }
        }
    }
    Ok(Json(out))
}

/// Render a content node as a v3 lookup item. Movie vs series field naming is
/// chosen by the node's media type, matching what the client expects.
fn v3_lookup_item(node: &cellarr_core::ContentNode, title: &str) -> Value {
    let base = json!({
        "title": title,
        "monitored": node.monitored,
        "qualityProfileId": Value::Null,
        "added": "0001-01-01T00:00:00Z",
        // The originals key on a metadata id; we surface our content id so an
        // add round-trips back to the right node.
        "id": node.id.to_string(),
    });
    match node.media_type {
        MediaType::Tv => merge(
            base,
            json!({ "tvdbId": 0, "seriesType": "standard", "titleSlug": slug(title) }),
        ),
        _ => merge(
            base,
            json!({ "tmdbId": 0, "year": 0, "titleSlug": slug(title) }),
        ),
    }
}

// --- add -------------------------------------------------------------------

/// v3 add body (the subset Overseerr sends). `qualityProfileId` and
/// `rootFolderPath` are required by the originals; `title` identifies the item.
#[derive(Debug, Deserialize)]
struct AddBody {
    title: Option<String>,
    #[serde(rename = "qualityProfileId")]
    quality_profile_id: Option<String>,
    #[serde(rename = "rootFolderPath")]
    root_folder_path: Option<String>,
    #[serde(default)]
    monitored: Option<bool>,
}

async fn add_movie(
    State(state): State<AppState>,
    Json(body): Json<AddBody>,
) -> ApiResult<Json<Value>> {
    add(&state, body, MediaType::Movie).await
}

async fn add_series(
    State(state): State<AppState>,
    Json(body): Json<AddBody>,
) -> ApiResult<Json<Value>> {
    add(&state, body, MediaType::Tv).await
}

async fn add(state: &AppState, body: AddBody, surface: MediaType) -> ApiResult<Json<Value>> {
    use cellarr_core::repo::ContentRepository;
    let title = body
        .title
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| ApiError::BadRequest("title is required".into()))?;

    // Pick a target library of the right type. The originals add into a single
    // app; cellarr maps to the first library matching the surface.
    let library = pick_library(state, surface).await?;

    // The quality profile: honor the body's id if valid, else the library
    // default — matching the originals, which require a profile on add.
    let profile_id = match body.quality_profile_id {
        Some(raw) if !raw.is_empty() => raw
            .parse::<uuid::Uuid>()
            .map(QualityProfileId::from_uuid)
            .map_err(|_| ApiError::BadRequest(format!("invalid qualityProfileId: {raw}")))?,
        _ => library.default_quality_profile,
    };

    // Create the structural root node (movie / series root) and index its title
    // so a subsequent lookup finds it — the real "add" effect.
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
    let node = cellarr_core::ContentNode {
        id: cellarr_core::ContentId::new(),
        library_id: library.id,
        media_type: surface,
        parent_id: None,
        kind,
        coords,
        monitored: body.monitored.unwrap_or(true),
        title_id: None,
    };
    let content = state.db.content();
    content.upsert(&node).await?;
    content.index_title(node.id, &title).await?;

    Ok(Json(merge(
        v3_lookup_item(&node, &title),
        json!({ "qualityProfileId": profile_id.to_string(),
                "rootFolderPath": body.root_folder_path }),
    )))
}

/// Find the first library of the requested type, or a structured 404 so the
/// client knows there is nowhere to add (matching the originals' "no root
/// folder" failure mode in spirit).
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

/// v3 command body: `{ "name": "...", "movieId"/"seriesId": ... }`.
#[derive(Debug, Deserialize)]
struct CommandBody {
    name: String,
    #[serde(rename = "movieId")]
    movie_id: Option<String>,
    #[serde(rename = "seriesId")]
    series_id: Option<String>,
}

async fn command(
    State(state): State<AppState>,
    Json(body): Json<CommandBody>,
) -> ApiResult<Json<Value>> {
    let content_id = body.movie_id.or(body.series_id);
    let kind = kind_for_command(&body.name, content_id)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown command: {}", body.name)))?;
    let cmd_name = command_name(&kind).to_string();
    let job_id = commands::submit(&state.scheduler, kind)
        .await
        .map_err(ApiError::Command)?;
    // The v3 command response shape the ecosystem polls on.
    Ok(Json(json!({
        "id": job_id,
        "name": body.name,
        "commandName": cmd_name,
        "status": "queued",
        "queued": "0001-01-01T00:00:00Z",
        "trigger": "manual",
    })))
}

// --- calendar / queue / history --------------------------------------------

/// v3 calendar — upcoming/aired items in a date window. cellarr has no air-date
/// table wired here yet, so this returns an empty (but correctly shaped) array;
/// clients that poll it (Notifiarr) tolerate an empty calendar.
async fn calendar() -> Json<Vec<Value>> {
    Json(Vec::new())
}

/// v3 queue — the paged `{ records: [...] }` envelope the ecosystem reads. We map
/// the scheduler's jobs into v3 queue records so a client sees in-flight work.
async fn queue(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let jobs = commands::list_jobs(&state.scheduler)
        .await
        .map_err(ApiError::Command)?;
    let records: Vec<Value> = jobs
        .into_iter()
        .map(|j| {
            json!({
                "id": j.id,
                "title": command_name(&j.kind),
                "status": format!("{:?}", j.state).to_ascii_lowercase(),
                "trackedDownloadStatus": "ok",
                "protocol": "unknown",
            })
        })
        .collect();
    Ok(Json(json!({
        "page": 1,
        "pageSize": records.len().max(1),
        "totalRecords": records.len(),
        "records": records,
    })))
}

/// v3 history — paged `{ records: [...] }`. History is per content node in
/// cellarr; absent a `?contentId=`/`?movieId=` we return an empty envelope.
#[derive(Debug, Deserialize)]
struct HistoryQuery {
    #[serde(rename = "movieId")]
    movie_id: Option<String>,
    #[serde(rename = "seriesId")]
    series_id: Option<String>,
}

async fn history(
    State(state): State<AppState>,
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
            state
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
    Ok(Json(json!({
        "page": 1,
        "pageSize": records.len().max(1),
        "totalRecords": records.len(),
        "records": records,
    })))
}

/// Map a cellarr history event onto a v3 `eventType` string the ecosystem reads.
fn history_event_type(event: &cellarr_core::HistoryEvent) -> String {
    // The serde tag of the event is a stable, descriptive token; reuse it.
    serde_json::to_value(event)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str().map(String::from)))
        .unwrap_or_else(|| "unknown".into())
}

// --- small JSON helpers ----------------------------------------------------

/// Shallow-merge object `b` into object `a` (b wins). Used to specialize a base
/// item with per-app fields.
fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(ao), Some(bo)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in bo {
            ao.insert(k.clone(), v.clone());
        }
    }
    a
}

/// A naive title slug for the v3 `titleSlug` field clients sometimes key on.
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
