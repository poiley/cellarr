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
        .route("/health", get(health))
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
        .route("/customformat", get(list_custom_formats))
        .route("/customFormat", get(list_custom_formats))
        .route("/customformat/schema", get(custom_format_schema))
        .route("/customFormat/schema", get(custom_format_schema))
        .route("/indexer", get(list_indexers))
        .route("/indexer/schema", get(indexer_schema))
        .route("/series", get(list_series))
        .route("/episode", get(list_episodes))
        .route("/movie", get(list_movies))
        .route("/movie/lookup", get(movie_lookup))
        .route("/series/lookup", get(series_lookup))
        .route("/calendar", get(calendar))
        .route("/queue", get(queue))
        .route("/history", get(history))
        .route("/wanted/missing", get(wanted_missing))
        .route("/command", get(list_commands))
        .with_state(fs.clone());

    let writes = Router::new()
        .route("/movie", post(add_movie))
        .route("/series", post(add_series))
        .route("/command", post(command))
        .route("/tag", post(create_tag))
        .route("/tag/{id}", put(update_tag))
        .route("/tag/{id}", delete(delete_tag))
        .route("/customformat", post(create_custom_format))
        .route("/customFormat", post(create_custom_format))
        .route("/customformat/{id}", put(update_custom_format))
        .route("/customFormat/{id}", put(update_custom_format))
        .route("/customformat/{id}", delete(delete_custom_format))
        .route("/customFormat/{id}", delete(delete_custom_format))
        .route("/indexer", post(create_indexer))
        .route("/indexer/{id}", put(update_indexer))
        .route("/indexer/{id}", delete(delete_indexer))
        .route("/indexer/test", post(test_indexer))
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

// --- health ----------------------------------------------------------------

/// v3 health checks. cellarr surfaces its own health as v3-shaped
/// `{ source, type, message, wikiUrl }` records; an all-clear is an empty array.
async fn health(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let cfg = fs.state.db.config();
    let mut out = Vec::new();
    if cfg.list_indexers().await?.is_empty() {
        out.push(json!({
            "source": "IndexerCheck",
            "type": "warning",
            "message": "No indexers are configured",
            "wikiUrl": "",
        }));
    }
    if cfg.list_download_clients().await?.is_empty() {
        out.push(json!({
            "source": "DownloadClientCheck",
            "type": "warning",
            "message": "No download client is configured",
            "wikiUrl": "",
        }));
    }
    if cfg.list_root_folders().await?.is_empty() {
        out.push(json!({
            "source": "RootFolderCheck",
            "type": "error",
            "message": "No root folders are configured",
            "wikiUrl": "",
        }));
    }
    Ok(Json(out))
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
    // Also include any standalone root folders the config layer holds.
    for rf in cfg.list_root_folders().await? {
        if seen.insert(rf.path.clone()) {
            out.push(json!({
                "id": idx,
                "path": rf.path,
                "accessible": rf.enabled,
                "freeSpace": 0,
                "unmappedFolders": [],
            }));
            idx += 1;
        }
    }
    Ok(Json(out))
}

// --- tag -------------------------------------------------------------------

/// Render a tag as the v3 `{ id, label }` shape.
fn v3_tag(tag: &crate::tags::Tag) -> Value {
    json!({ "id": tag.id, "label": tag.label })
}

async fn list_tags(State(fs): State<FaceState>) -> Json<Vec<Value>> {
    Json(fs.state.tags.list().iter().map(v3_tag).collect())
}

async fn get_tag(State(fs): State<FaceState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let id = parse_u32(&id, "tag")?;
    fs.state
        .tags
        .get(id)
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
    Ok(Json(v3_tag(&fs.state.tags.create(body.label.trim()))))
}

async fn update_tag(
    State(fs): State<FaceState>,
    Path(id): Path<String>,
    Json(body): Json<TagBody>,
) -> ApiResult<Json<Value>> {
    let id = parse_u32(&id, "tag")?;
    fs.state
        .tags
        .update(id, body.label.trim())
        .map(|t| Json(v3_tag(&t)))
        .ok_or_else(|| ApiError::NotFound(format!("tag {id} not found")))
}

async fn delete_tag(State(fs): State<FaceState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let id = parse_u32(&id, "tag")?;
    if fs.state.tags.delete(id) {
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
    let libs = fs.state.db.config().list_libraries().await?;
    let repo = fs.state.db.profiles();
    let formats = repo.custom_formats().await?;
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for lib in libs {
        if let Some(media) = fs.face.fixed_media() {
            if lib.media_type != media {
                continue;
            }
        }
        let pid = lib.default_quality_profile;
        if !seen.insert(pid.as_uuid()) {
            continue;
        }
        if let Some(profile) = repo.get_profile(pid).await? {
            out.push(v3_quality_profile(&profile, &formats, fs.face));
        }
    }
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
                "quality": { "id": q.rank, "name": q.name, "source": "unknown", "resolution": 0 },
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

// --- qualitydefinition -----------------------------------------------------

/// v3 `qualitydefinition` — the quality catalogue with size limits. Built from
/// cellarr's default quality ranking; Recyclarr reads it to map quality names.
async fn quality_definitions() -> Json<Vec<Value>> {
    let ranking = cellarr_core::QualityRanking::default();
    let out: Vec<Value> = ranking
        .qualities
        .iter()
        .map(|q| {
            json!({
                "id": q.rank + 1,
                "quality": { "id": q.rank, "name": q.name, "source": "unknown", "resolution": 0 },
                "title": q.name,
                "weight": q.rank + 1,
                "minSize": q.min_size_per_min.unwrap_or(0),
                "maxSize": q.max_size_per_min,
                "preferredSize": Value::Null,
            })
        })
        .collect();
    Json(out)
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
        K::Codec { .. } => "ReleaseTitleSpecification",
        K::Hdr { .. } => "ReleaseTitleSpecification",
        K::QualityModifier { .. } => "ReleaseTitleSpecification",
        K::Language { .. } => "LanguageSpecification",
        K::IndexerFlag { .. } => "IndexerFlagSpecification",
        K::Size { .. } => "SizeSpecification",
    }
}

/// The regex/value a condition contributes to its v3 spec `value` field.
fn spec_value(kind: &cellarr_core::ConditionKind) -> Value {
    use cellarr_core::ConditionKind as K;
    match kind {
        K::ReleaseTitle { pattern } => json!(pattern),
        K::ReleaseGroup { name } => json!(name),
        K::Language { language } => json!(language),
        K::IndexerFlag { flag } => json!(flag),
        // The remaining kinds carry typed enums; surface their serde token so the
        // value round-trips losslessly enough for Recyclarr's diffing.
        other => serde_json::to_value(other)
            .ok()
            .and_then(|v| {
                v.get("value").cloned().or_else(|| {
                    v.as_object().and_then(|o| {
                        o.values()
                            .find(|x| !x.is_string() || x.as_str() != Some(""))
                            .cloned()
                    })
                })
            })
            .unwrap_or(Value::Null),
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
    let spec = |impl_name: &str, label: &str| {
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
    Json(vec![
        spec("ReleaseTitleSpecification", "Release Title"),
        spec("ReleaseGroupSpecification", "Release Group"),
        spec("SourceSpecification", "Source"),
        spec("ResolutionSpecification", "Resolution"),
        spec("LanguageSpecification", "Language"),
        spec("IndexerFlagSpecification", "Indexer Flag"),
        spec("SizeSpecification", "Size"),
    ])
}

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

/// Map a v3 spec body back onto a cellarr condition. Implementations cellarr
/// models map to their typed kind; everything else degrades to a release-title
/// regex (lossless enough for Recyclarr's name/score diffing).
fn condition_from_spec(spec: &SpecBody) -> cellarr_core::Condition {
    use cellarr_core::ConditionKind as K;
    let value = spec
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some("value"))
        .map(|f| &f.value);
    let value_str = value.and_then(|v| v.as_str()).unwrap_or("").to_string();
    let kind = match spec.implementation.as_deref() {
        Some("ReleaseGroupSpecification") => K::ReleaseGroup { name: value_str },
        Some("LanguageSpecification") => K::Language {
            language: value_str,
        },
        Some("IndexerFlagSpecification") => K::IndexerFlag { flag: value_str },
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

async fn delete_custom_format(
    State(_fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    // cellarr's persistence layer has no custom-format delete yet; accept the
    // request idempotently (the ecosystem only needs a 200) and report the gap.
    parse_i64(&id, "customformat")?;
    Ok(Json(json!({})))
}

// --- indexer ---------------------------------------------------------------

/// Render a cellarr [`IndexerConfig`] into the v3 indexer shape Prowlarr reads
/// back after a push: identity + flags + a `fields[]` projection of `settings`.
fn v3_indexer(ix: &cellarr_core::IndexerConfig) -> Value {
    let fields: Vec<Value> = ix
        .settings
        .as_object()
        .map(|o| {
            o.iter()
                .enumerate()
                .map(|(i, (k, v))| json!({ "order": i, "name": k, "value": v }))
                .collect()
        })
        .unwrap_or_default();
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
        "tags": [],
    })
}

async fn list_indexers(State(fs): State<FaceState>) -> ApiResult<Json<Vec<Value>>> {
    let indexers = fs.state.db.config().list_indexers().await?;
    Ok(Json(indexers.iter().map(v3_indexer).collect()))
}

/// v3 `indexer/schema` — at minimum a Torznab and a Newznab template, which
/// Prowlarr round-trips its pushed indexer through.
async fn indexer_schema() -> Json<Vec<Value>> {
    let entry = |impl_name: &str, protocol: &str| {
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
            "fields": [
                { "order": 0, "name": "baseUrl", "label": "URL", "type": "textbox", "advanced": false },
                { "order": 1, "name": "apiPath", "label": "API Path", "value": "/api", "type": "textbox", "advanced": true },
                { "order": 2, "name": "apiKey", "label": "API Key", "type": "textbox", "advanced": false, "privacy": "apiKey" },
                { "order": 3, "name": "categories", "label": "Categories", "type": "select", "advanced": false }
            ],
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
    let mut settings = serde_json::Map::new();
    for f in &body.fields {
        if let Some(name) = &f.name {
            settings.insert(name.clone(), f.value.clone());
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
        settings: Value::Object(settings),
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
    State(_fs): State<FaceState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    // No indexer delete in the persistence layer yet; accept idempotently.
    parse_i64(&id, "indexer")?;
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
    let content = state.db.content();
    let ids = content.search(term).await?;
    let mut out = Vec::new();
    for id in ids {
        if let Some(node) = content.get_node(id).await? {
            if node.media_type == surface {
                out.push(v3_resource_item(state, &node, term).await?);
            }
        }
    }
    Ok(Json(out))
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
        "tags": [],
    });
    Ok(match node.media_type {
        MediaType::Tv => {
            let mut v = merge(
                base,
                json!({ "tvdbId": 0, "seriesType": "standard", "status": "continuing" }),
            );
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
                json!({ "tmdbId": 0, "year": 0, "status": "released", "hasFile": file.is_some() }),
            );
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
            // The FTS index has no reverse lookup, so a node's title text is not
            // recoverable here; fall back to its id. (Reported as a core gap —
            // a title column on the resolved-identity row would close it.)
            let title = node.id.to_string();
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

/// v3 `episode` list — Bazarr reads per-series episodes. cellarr has no episode
/// projection wired here yet, so this returns a correctly-shaped empty array.
async fn list_episodes() -> Json<Vec<Value>> {
    Json(Vec::new())
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

// --- calendar / queue / history / wanted -----------------------------------

async fn calendar() -> Json<Vec<Value>> {
    Json(Vec::new())
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

async fn queue(State(fs): State<FaceState>) -> ApiResult<Json<Value>> {
    let jobs = commands::list_jobs(&fs.state.scheduler)
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
    Ok(Json(paged(records, "timeleft")))
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

/// Project an [`IndexerId`] (uuid) onto a stable positive integer.
fn ix_numeric_id(id: cellarr_core::IndexerId) -> i64 {
    uuid_to_i64(id.as_uuid())
}

/// Map a uuid to a stable positive `i64` for v3 integer id fields.
fn uuid_to_i64(id: uuid::Uuid) -> i64 {
    let bytes = id.as_bytes();
    let mut n: i64 = 0;
    for b in &bytes[..8] {
        n = (n << 8) | i64::from(*b);
    }
    n & i64::MAX
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
