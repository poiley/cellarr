# Spec: cellarr-llm

## Responsibility
Optional inference fallback for **parsing** and **identify** when deterministic confidence is low.
**Local-first** and entirely optional — the daemon must be fully functional with this disabled.
Never authoritative for destructive operations.

## Allowed dependencies
Internal: `cellarr-core`. External: a local inference client (e.g. llama.cpp/Ollama bindings or an
HTTP client to a local model server), optional cloud-provider client behind a feature, `serde`,
`thiserror`, `moka` (result cache). All providers behind cargo features; **none required by default**.

## Public interface
- `parse_fallback(title, context) -> Option<ParsedRelease>` — structured-output parse with confidence.
- `match_fallback(parsed, candidates) -> Option<ContentMatch>` — disambiguation aid.
- A provider abstraction (local model, optional remote) selected by config.

## Behavior
- **Offline non-negotiable:** a local model path must exist; cloud is an optional enhancement only.
- **Cached** by normalized input so any given hard case costs at most one inference ever.
- **Confidence-gated and never destructive:** results may inform a grab suggestion, but a
  low-confidence/inference-derived match that would replace/delete a file is held for user
  confirmation ([03-pipeline.md](../03-pipeline.md)). A hallucinated parse must never overwrite the
  wrong file.
- Inference results confirmed by a successful import are candidates for promotion into `/corpus`
  (with human review), so the deterministic parser keeps improving.
- Structured output only (schema-constrained); free-form text is not trusted.

## Test obligations
- With the feature **disabled**, the whole system builds and all non-LLM tests pass (proves optional).
- Cached-fixture tests: given a recorded model response, the fallback returns the expected structured
  result; cache hit avoids a second call.
- Confidence-gate tests: low-confidence destructive paths are held, not executed.
- Schema-violation handling: malformed model output is rejected, not trusted.

## References
[04-parser.md](../04-parser.md), [03-pipeline.md](../03-pipeline.md).
