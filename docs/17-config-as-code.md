# 17 — Configuration as code (managed config)

cellarr can be configured **declaratively from a single YAML file** committed to git, instead of
(or alongside) clicking through the settings UI. On boot the daemon reconciles its database to match
that file: it creates what is declared, updates what drifted, prunes what it previously managed but
that is no longer declared, and leaves everything you created in the UI untouched. This is the same
idea as [Recyclarr](https://recyclarr.dev/) for custom formats, but native, broader (it covers the
whole management surface, not just CFs/quality profiles), and enforced at startup rather than by an
external cron.

It is **opt-in and additive**: with no managed-config path set, startup behaviour is exactly as
before (zero-config, SQLite, works offline). Nothing about the `/api/v3` shim changes except one
**read-only, additive** `managed: true` field on managed resources (see [The `managed` flag](#the-managed-flag)).

> This is a parity *superset*: Sonarr/Radarr have no first-party config-as-code. See
> [`parity/REPLACEMENT-ROADMAP.md`](parity/REPLACEMENT-ROADMAP.md).

---

## Turning it on

Point cellarr at a file with the `CELLARR_MANAGED_CONFIG_PATH` environment variable (or the
equivalent `managed_config_path` key in the config file):

```sh
CELLARR_MANAGED_CONFIG_PATH=/config/managed-config.yaml cellarr serve
```

When set, the daemon — **after migrations, before it serves a single request** — loads the file,
reconciles the DB to match, and logs a summary (`created`, `updated`, `pruned`, `unchanged`). When
unset, this whole step is a no-op.

A complete, heavily-commented example with every section populated lives at
[`deploy/managed-config.example.yaml`](../deploy/managed-config.example.yaml). A Kubernetes
deployment that mounts the file from a `ConfigMap` and feeds secrets from `Secret`s is in
[`deploy/k8s/cellarr.yaml`](../deploy/k8s/cellarr.yaml).

---

## Fail-loud boot

The reconcile runs **before serving**, and **any error fails boot** — the daemon refuses to start
rather than serve a stale or half-applied configuration. Validation is deliberately strict:

- **Unknown fields are hard errors.** Every struct is `deny_unknown_fields`, so a typo like `apikey`
  (for `apiKey`) or `qualtiyProfile` is a startup failure naming the bad key — never a silently
  dropped field that leaves the daemon subtly misconfigured.
- **The `apiVersion` is gated.** A file declaring an `apiVersion` this build doesn't understand is
  rejected, so a forward/backward-incompatible config fails loudly instead of being half-applied.
  This build understands `cellarr/v1`.
- **A missing required secret fails before parsing.** A `${VAR}` reference with no value (and no
  default) errors and names the variable, so a misconfigured deployment never silently authenticates
  with an empty API key.
- **Cross-references are checked.** A library naming a non-existent quality profile, a profile
  referencing an undeclared custom format, a duplicate name within a section, an `auth` block that
  selects an enforcing method with no credential (which would lock you out) — all are caught at load
  time, before anything is applied.

Because validation and the (read-only) plan can be run **without** booting the daemon (see
[The CLI](#the-cli)), you can gate a config change in CI before it ever reaches production.

---

## The file

The file is a single YAML document. The only required key is `apiVersion`; `version` is an optional
operator-facing label surfaced in logs. Every other top-level key is an **optional section**. Each
section is a list of items keyed by a stable, human **`name`** — that name is the reconcile identity,
so renaming an item is a delete-and-recreate, while editing its other fields is an in-place update.

```yaml
apiVersion: cellarr/v1
version: "2026-06-26.1"   # optional, informational
# ... sections ...
```

### Sections

The sections, in the dependency order reconciliation applies them (a thing is created before
whatever references it):

| Section | What it manages | Notable references |
|---|---|---|
| `tags` | The tag vocabulary (label → tag) | — |
| `qualityDefinitions` | Per-quality title + size bounds (bytes/min) | quality name must exist in the catalogue |
| `customFormats` | Named, scored condition bundles (TRaSH-style) | — |
| `qualityProfiles` | Allowed qualities + cutoff + CF score thresholds | qualities by name; `customFormatScores` by CF name |
| `rootFolders` | Import root folders | — |
| `libraries` | Libraries (per media type) | `rootFolders` by name; `qualityProfile` by name |
| `indexers` | Torznab/Newznab/… with nested v3-style `settings` | `tags` by name |
| `downloadClients` | qBittorrent/SABnzbd/Deluge/rTorrent/blackhole/… | `tags` by name |
| `releaseProfiles` | Required/ignored/preferred terms, tag-scoped | `tags` by name |
| `delayProfiles` | Per-protocol grab delays + preference | `tags` are opaque labels |
| `importLists` | Trakt/TMDb/Plex/IMDb, with nested `settings` | `qualityProfile` by name (optional) |
| `notifications` | Discord/webhook/… targets, tag-scoped | `tags` by name |
| `remotePathMappings` | download-client path → cellarr-visible path | — |
| `naming` | Library-wide naming templates (a singleton) | — |
| `mediaManagement` | Recycle-bin / permissions / extra-files (a singleton) | — |
| `auth` | The single-admin web-UI auth row (a singleton) | password supplied **pre-hashed** |

The exact field names for every section are the source of truth in the schema
(`crates/cellarr-cli/src/managed/schema.rs`) and are shown filled-in in
[`deploy/managed-config.example.yaml`](../deploy/managed-config.example.yaml). They are the **same
field names the `/api/v3` write shape uses** (`baseUrl`, `apiKey`, host/port/category, …), so an
indexer or download-client definition transcribes directly from the *arr UI or a Recyclarr config.
Indexer/client/list/notification `settings` are a native nested YAML map, not an escaped string.

### A note on adapter `settings`

`indexers`, `downloadClients`, `importLists`, and `notifications` each carry a free-form `settings`
map (the adapter-specific fields: `baseUrl`, `apiKey`, host/port/credentials, a Trakt list slug, a
webhook URL, …). Those are passed through to the adapter rather than schema-checked field by field —
so the strictness there is the adapter's, and a string value supports `${ENV}` interpolation for the
secrets it contains.

---

## Secret interpolation

A managed-config file is committed to git, so secrets must **never** appear in it literally. Any
string value can carry an environment reference, resolved against the process environment **before**
the YAML is parsed:

| Form | Meaning |
|---|---|
| `${VAR}` | The value of `VAR`. **Error** (fails boot, names `VAR`) if `VAR` is unset. |
| `${VAR:-default}` | The value of `VAR`, or `default` if `VAR` is unset **or empty**. |
| `$$` | A literal `$` (the only escape — lets a value contain a real dollar sign). |
| `$` (bare) | Passed through untouched, so prices, regex anchors (`^foo$`), etc. survive. |

```yaml
indexers:
  - name: nzbgeek
    kind: newznab
    protocol: usenet
    settings:
      baseUrl: https://api.nzbgeek.info   # public config — committed verbatim
      apiKey: ${NZBGEEK_API_KEY}          # secret — injected from the environment
```

In Kubernetes you wire the real values in via `env: valueFrom: secretKeyRef` (see the sample
deployment); locally they come from your shell or a gitignored `.env`. Because interpolation runs on
the **raw text**, it is uniform across every section regardless of which field a secret sits in.

---

## What reconciliation does (and does not) prune

This is the part to understand before turning it on. cellarr tracks, in a **ledger table**
(`managed_config_entity`), every entity it created from config, keyed by `(kind, entity_id)`.
Reconciliation is scoped entirely to that ledger:

- **A section that is present and lists items** is authoritative for the entities *config previously
  created*: items in the file are created/updated, and any ledger entity of that kind no longer in
  the file is **pruned**.
- **A section that is present but empty (`indexers: []`)** means "manage this kind, declaring none" —
  it prunes every config-created entity of that kind. This is the explicit "delete them all" form.
- **A section that is omitted entirely** means "do not manage this kind at all" — that kind is left
  completely untouched. (Omitting a section and declaring it empty are deliberately different.)
- **UI-created entities are never pruned.** Pruning only ever targets ledger rows. An indexer you
  added by hand in the UI has no ledger row, so config never deletes it — config and the UI
  coexist on the same instance.

Reconciliation is **idempotent**: re-running the same file is a no-op (every item hashes to its
recorded hash → zero changes, zero prunes). The singleton sections (`naming`, `mediaManagement`,
`auth`) set the whole document when declared and are left untouched when omitted.

### The `managed` flag

Every managed resource carries a **read-only, additive** `managed: true` boolean on its `/api/v3`
representation (derived purely from the ledger; a UI-created entity reads `managed: false`). The web
UI uses it to badge and lock config-managed entities — editing them in the UI would just be reverted
on the next reconcile, so the UI steers you to the file instead. No existing field changes and the
flag is additive, so *arr ecosystem clients that ignore the extra key are unaffected.

---

## The CLI

Two subcommands let you work with a managed config without booting the daemon. Both open the
configured database (read-only for these operations) under the data dir, and both resolve their
target file from `--file PATH`, else `CELLARR_MANAGED_CONFIG_PATH` / the config `managed_config_path`.

```sh
# Load + interpolate + validate the file, then compute the reconcile plan against
# the live DB and print the diff. Exit code is distinct on drift, so this gates CI.
cellarr managed-config validate --file ./managed-config.yaml

# Dump the current DB state of every managed-able kind as a managed-config YAML
# document (the inverse of reconcile). Secrets are emitted as ${ENV} placeholders.
cellarr managed-config export --file ./managed-config.yaml   # omit --file to print to stdout
```

`validate` exit codes: `0` clean (live config matches the file), a distinct non-zero on **drift**
(the file would change something), and a distinct non-zero on a **config error** (invalid file). The
config-error case is what catches a typo or a missing secret in CI.

**`export` never emits secrets.** A `settings` value whose key looks secret (`apiKey`, `password`,
`passkey`, `secret`, `token`, `webhook`, …) is replaced with a `${ENV}` placeholder derived from the
entity and field, so the exported file is safe to commit and you wire the real secret into the
environment. (`baseUrl` is *not* treated as secret — it is public config that round-trips verbatim.)

---

## The recommended workflow: configure-in-UI, then export

You don't have to hand-author the whole file. The intended path for an existing instance is:

1. **Configure in the UI** as you normally would — add indexers, build quality profiles, import your
   TRaSH custom formats, set up notifications.
2. **`cellarr managed-config export`** to capture that state into a YAML file. Secrets come out as
   `${ENV}` placeholders.
3. **Review and commit** the file to git. Wire the real secrets into your deployment's environment
   (k8s `Secret`s, a gitignored `.env`, …).
4. **Set `CELLARR_MANAGED_CONFIG_PATH`** to the file. From now on the file is the source of truth:
   those entities show as `managed` in the UI, drift is reconciled on every boot, and you change
   configuration by editing the file (gating it with `managed-config validate` in CI) rather than
   clicking.

The export round-trips: feeding an exported file back through `validate`/reconcile produces an empty
plan. You can keep using the UI for anything you *don't* put under management — the two coexist.

---

## See also

- [`deploy/managed-config.example.yaml`](../deploy/managed-config.example.yaml) — the complete,
  commented sample with every section.
- [`deploy/k8s/cellarr.yaml`](../deploy/k8s/cellarr.yaml) — a Kubernetes deployment that mounts the
  config from a `ConfigMap` and feeds secrets from `Secret`s.
- `crates/cellarr-cli/src/managed/` — the engine (schema, interpolate, loader, validate, plan,
  reconcile, export).
- [09-api.md](09-api.md) — the `/api/v3` shim the `managed` flag rides on.
