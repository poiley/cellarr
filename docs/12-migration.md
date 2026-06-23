# 12 — Migration from existing *arr installs

**No migration path = no adoption.** A current Radarr/Sonarr/Lidarr user must be able to bring their
library and config into cellarr without re-scanning terabytes or re-matching everything by hand.
`cellarr-migrate` is a **day-one feature**, not a someday-feature.

## What we import

From a user's existing app database(s) and config:

- **Library structure & identity:** monitored movies/series/seasons/episodes (and music/books
  later), with their external IDs (tmdb/tvdb/imdb/musicbrainz) so we don't re-identify from scratch.
- **File associations:** which files satisfy which items, current quality, and on-disk paths — so
  the existing library is recognized in place, not re-imported.
- **Quality profiles & custom formats:** mapped onto cellarr's decision model
  ([05-decision-engine.md](05-decision-engine.md)). TRaSH-style CFs import directly.
- **Indexers & download clients:** connection settings (re-test on import).
- **History** (best-effort) so the user keeps their record.
- **Root folders, naming schemes, tags.**

## How it works

- The originals use **SQLite** by default (Postgres optionally). `cellarr-migrate` reads the source
  database **read-only** (never mutates the user's existing install) and maps rows into cellarr's
  schema ([02-data-model.md](02-data-model.md)).
- Run as a guided import in first-run onboarding, or via CLI. The user points at their existing
  config/DB location(s); cellarr previews what will be imported before committing.
- Because cellarr is unified, importing both a Radarr and a Sonarr DB produces **one** library set
  with movies and TV side by side.
- Alternatively, a user can keep their existing Prowlarr and have cellarr **consume it as an indexer
  source** during transition (see [06-integrations.md](06-integrations.md)).

## Safety

- Source DB opened read-only; the user's existing app is untouched and can keep running during
  evaluation.
- Import is previewed and reversible (it's a fresh cellarr DB; throw it away and re-import).
- File-system: import **recognizes existing files in place** and must never move/delete during
  migration — the destructive pipeline only runs on future grabs.

## Testing

- Fixture databases: small, sanitized example Radarr/Sonarr SQLite DBs (schema-representative,
  no personal data) → asserted mapping into cellarr's schema.
- Round-trip checks: counts and key identities preserved; profiles/CFs produce equivalent decisions
  (verified via the decision corpus).
- A "recognize in place" test: given a library tree + an imported DB, assert no file operations are
  scheduled for already-correct files.

See [`specs/cellarr-migrate.md`](specs/cellarr-migrate.md). Note: the originals consider their own
SQLite→Postgres migration "new installs only" — that's a different concern; our cross-app importer
is explicitly supported and tested.
