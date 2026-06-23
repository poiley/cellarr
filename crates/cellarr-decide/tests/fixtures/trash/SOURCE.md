# TRaSH-Guides custom-format fixtures (test data — attribution)

These JSON files are **test data**, not a vendored part of cellarr.

- **Source:** [TRaSH-Guides/Guides](https://github.com/TRaSH-Guides/Guides) — the
  community-maintained Servarr configuration guide.
- **Files:** the Sonarr custom-format set (`docs/json/sonarr/cf/*.json`) and the Radarr
  custom-format set (`docs/json/radarr/cf/*.json`), copied verbatim in the Servarr export
  shape (`name` + `specifications[]` + `trash_id` + `trash_scores`).
- **Provenance:** cloned at commit `020dd0c42e92e1b815d40263bf545f9bd2a365d3`
  (2026-06-23). Refresh by re-cloning `TRaSH-Guides/Guides` into `reference/` and recopying.
- **`scores.default.json`:** a derived map (`trash_id` → recommended score) extracted from each
  CF's `trash_scores.default` flavor (0 when that CF has no `default` score). This is the score
  source cellarr's importer consumes; it is a projection of the upstream data, not new content.

## Terms / why this is OK to use as test data

This is freely-consumable configuration data: the whole point of TRaSH-Guides is for tools like
Recyclarr / Configarr (and now cellarr) to **consume** these CF definitions. We use them here
**only** to test cellarr's importer (`import_trash_custom_formats*`) — verifying that real-world
TRaSH CFs import and that the supported ones compile into a valid `MatchContext`.

We do **not** relicense or republish this data as part of cellarr, and we do **not** copy upstream
*code*. The TRaSH-Guides repository is licensed for use under its own terms; consult the upstream
repo (its `LICENSE`) for the authoritative license. Treat this directory as a cached copy of an
external, attributed source.
