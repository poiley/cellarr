//! The OpenAPI spec for the native API.
//!
//! Generated from one source-of-truth builder rather than a heavyweight derive
//! dependency, keeping the default build lean and fully offline (a
//! non-negotiable). The spec is served at `/api/v1/openapi.json` and consumed by
//! the web UI and external clients. It is kept in lock-step with [`crate::native`]
//! by the openapi test, which asserts every native route has a path entry.

use serde_json::{json, Value};

/// The list of native `/api/v1` paths the spec must document, paired with the
/// methods they expose. The router and this list are checked against each other
/// in tests so they cannot drift.
pub const NATIVE_PATHS: &[(&str, &[&str])] = &[
    ("/api/v1/system/status", &["get"]),
    ("/api/v1/libraries", &["get", "post"]),
    ("/api/v1/libraries/{id}", &["get"]),
    ("/api/v1/libraries/{id}/content", &["get"]),
    ("/api/v1/content/{id}", &["get"]),
    ("/api/v1/content/{id}/files", &["get"]),
    ("/api/v1/content/{id}/history", &["get"]),
    ("/api/v1/indexers", &["get", "post"]),
    ("/api/v1/downloadclients", &["get", "post"]),
    ("/api/v1/qualityprofiles", &["get"]),
    ("/api/v1/qualityprofiles/{id}", &["get"]),
    ("/api/v1/customformats", &["get"]),
    ("/api/v1/queue", &["get"]),
    ("/api/v1/history", &["get"]),
    ("/api/v1/decisionlog/{run_id}", &["get"]),
    ("/api/v1/commands", &["get", "post"]),
    ("/api/v1/stream", &["get"]),
    ("/api/v1/openapi.json", &["get"]),
];

/// Build the OpenAPI 3.1 document.
#[must_use]
pub fn spec() -> Value {
    let mut paths = serde_json::Map::new();
    for (path, methods) in NATIVE_PATHS {
        let mut ops = serde_json::Map::new();
        for method in *methods {
            ops.insert(
                (*method).to_string(),
                json!({
                    "summary": format!("{} {}", method.to_uppercase(), path),
                    "security": security_for(method),
                    "responses": {
                        "200": { "description": "Success" },
                        "400": { "$ref": "#/components/responses/Error" },
                        "401": { "$ref": "#/components/responses/Error" },
                        "404": { "$ref": "#/components/responses/Error" },
                    }
                }),
            );
        }
        paths.insert((*path).to_string(), Value::Object(ops));
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "cellarr native API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Native cellarr REST + SSE API. The /api/v3 Radarr/Sonarr \
                            compatibility shim is documented separately as an external contract.",
        },
        "servers": [{ "url": "/" }],
        "paths": Value::Object(paths),
        "components": {
            "securitySchemes": {
                "ApiKeyHeader": { "type": "apiKey", "in": "header", "name": "X-Api-Key" },
                "ApiKeyQuery": { "type": "apiKey", "in": "query", "name": "apikey" },
            },
            "responses": {
                "Error": {
                    "description": "Structured error body",
                    "content": { "application/json": { "schema": {
                        "$ref": "#/components/schemas/ApiError" } } },
                }
            },
            "schemas": {
                "ApiError": {
                    "type": "object",
                    "required": ["code", "message"],
                    "properties": {
                        "code": { "type": "string",
                                  "description": "Stable machine-readable error code." },
                        "message": { "type": "string",
                                     "description": "Human-readable detail." },
                    },
                }
            },
        },
    })
}

/// Mutating methods carry the API-key security requirement; reads do not.
fn security_for(method: &str) -> Value {
    if matches!(method, "post" | "put" | "patch" | "delete") {
        json!([{ "ApiKeyHeader": [] }, { "ApiKeyQuery": [] }])
    } else {
        json!([])
    }
}
