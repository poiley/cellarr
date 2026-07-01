# 18 — Observability

How cellarr emits logs, spans, and (opt-in) OpenTelemetry traces, and the naming
conventions every crate follows so a run is legible end to end. The guiding
principle matches the rest of the plan: **works offline, zero required services.**
Structured logging is always on and writes to a local file; exporting to an OTLP
collector is strictly opt-in and off by default, so the default static binary
neither dials out nor links a network exporter it will not use.

## Logging (always on)

The binary initializes a [`tracing`](https://docs.rs/tracing) subscriber in
`cellarr-cli` (`init_tracing`): an `EnvFilter` (overridable at the process
boundary with `RUST_LOG`), a console layer, and — when a data dir is configured —
a **rolling daily file appender** at `<data_dir>/logs/cellarr.log`. That file is
what the `/api/v3/log/file` surface reads back. Nothing here requires a network.

Every crate logs through `tracing` macros (`info!`, `warn!`, `error!`, `debug!`),
never `println!`. Structured fields (`key = value`) are preferred over
interpolated strings so a log line is machine-parseable and a field lines up with
the same field on its span.

## Spans

Instrumented functions open a span named for the operation, carrying the
correlation fields below. Spans nest: a `pipeline.run` is the parent of the
`pipeline.discover` / `pipeline.decide` / `pipeline.grab` / `pipeline.track` /
`pipeline.import` it drives, and a grab's `download.add` is a child of the grab
span. When the OTLP layer is enabled this nesting becomes a distributed trace;
when it is not, the same fields still tag every log line emitted within the span.

### Span catalogue

| Span | Where | Key fields |
|------|-------|-----------|
| `pipeline.run` | `cellarr-jobs` runner | `content_id`, `run_id` |
| `pipeline.discover` | runner | `content_id` |
| `pipeline.decide` | runner | `content_id`, `release`, `indexer` |
| `pipeline.grab` | runner | `run_id`, `content_id`, `release`, `indexer` |
| `pipeline.track` | runner | `download_id` |
| `pipeline.import` | runner | `grab_id`, `content_id` |
| `download.add` | `cellarr-download` (transmission) | `download_client`, `release`, `indexer` |
| `download.status` | transmission | `download_client`, `download_id` |
| `download.remove` | transmission | `download_client`, `download_id`, `delete_data` |
| `scheduler.tick` | `cellarr-jobs` scheduler | — |
| `scheduler.job` | scheduler | `job_kind`, `job_id` |

Incoming HTTP requests are already spanned by `tower_http`'s `TraceLayer`
(installed in `cellarr-api`), which opens a `request` span per call carrying the
method and path — so the API surface is covered without per-handler
instrumentation.

## Canonical field names

Use these exact names so a field means the same thing on every span and log line
and a collector can correlate across stages. Prefer adding a field over baking a
value into a message string.

| Field | Meaning |
|-------|---------|
| `content_id` | The `content` node a run/stage concerns |
| `run_id` | One pipeline run (`PipelineRunId`), correlates every stage of a run |
| `grab_id` | A persisted grab |
| `release` | The release title under consideration |
| `indexer` | The indexer id a release came from |
| `download_client` | The download client kind (e.g. `transmission`) |
| `download_id` | The download client's own id/hash for a download |
| `delete_data` | Whether a removal also deletes the downloaded data |

## OpenTelemetry export (opt-in)

Off by default. Build with the `otlp` feature and set `CELLARR_OTEL__ENDPOINT` to
a collector's OTLP/HTTP endpoint to export the spans above as traces. The
exporter uses OTLP over **HTTP/protobuf** (not gRPC) so it links `reqwest` with
the rustls stack already in the tree rather than pulling in `tonic`/gRPC, keeping
the opt-in build close to the default one. Resource attributes identify the
service: `service.name = cellarr`, `service.version` (crate version), and a
per-process `service.instance.id`.

When the feature is off, none of the OpenTelemetry crates are compiled and the
binary has no network exporter — the file/console logging above is the whole
story.
