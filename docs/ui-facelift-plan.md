# cellarr UI Facelift Plan — SRCL-Composed, Terminal-Dense, Theme-Correct

> **Hard constraint honored throughout:** every recommendation composes **existing** SRCL (`reference/www-sacred`) components. No new primitives. Light + dark + system + tint must all work via `--theme-*`/`--ansi-*` tokens. Catalog: `/Users/poile/repos/cellarr/reference/www-sacred/components/AGENTS.md`; tokens: `/Users/poile/repos/cellarr/reference/www-sacred/global.css`.

---

## 1. Executive Summary & Core Diagnosis

cellarr already has the right bones: a per-page `AppShell` (SidebarLayout + Navigation), a working command palette, a genuinely solid theming layer (flash-free prehydration, system mode, OKLCH tint math, monospace ch-grid), and broad use of the SRCL workhorses (`Text`, `Card`, `Badge`, `Divider`, `Button`, `Input`). The UI feels weak not because the foundation is wrong but because **the app composes a narrow slice of SRCL and reinvents the rest by hand**, flattening exactly the data a media manager exists to show.

### The 3–5 systemic reasons it underperforms

| # | Systemic root cause | Where it shows | Dimensions hit |
|---|---|---|---|
| **D1** | **The app is effectively monochrome.** SRCL `Badge` is gray-only by construction, and every status — MONITORED/MISSING/DOWNLOADED, error/warning, quality, queue lifecycle, decision verdicts — renders as identical gray chips or a single glyph (`▲`/`●`/`✓`). The most load-bearing signal (severity/state) carries zero visual weight, **even though SRCL ships theme-aware ANSI tokens** (`--ansi-9-red`, `-2-green`, `-10-lime`, `-11-yellow`, `-12-blue`) and `SimpleTable` has a status-color contract. | Dashboard, Library, Activity, Calendar, Content, Decision-log, Logs | hierarchy, states, clarity |
| **D2** | **High-value SRCL primitives are never imported; their jobs are hand-rolled.** 27 components are unused, including `DropdownMenu`/`DropdownMenuTrigger` (per-row actions), `ActionBar` (toolbars), `RowEllipsis` (truncation), `DatePicker` (calendar), `TextArea`, `Drawer`, `Tooltip`/`Popover`. So per-row actions are bare Button stacks or bulk-only flows, tooltips are inaccessible native `title=` attrs, and the dashboard StatTiles / logs `<pre>` / settings connector `<ul>`s bypass SRCL tables entirely with raw `1px solid var(--theme-border)` divs. | App-wide (StatTile, Notifications, Integration, logs, queue) | rich-data, consistency, a11y |
| **D3** | **No shared data-row contract → three competing list renderers.** Semantic `Table` (13 files, no scroll wrapper), string-only `SimpleTable` (2 files, can't hold Badges/actions/media-status coloring), and hand-rolled `<div>`/`RowSpaceBetween`/`<ul>`/`<pre>` stacks coexist — sometimes on the **same page** (Activity: Downloads = divs, Self-heal = Table). Columns that the API already returns (protocol, indexer, ETA/timeleft, quality, episode progress) are dropped. | Activity, Library, History, Import, Interactive, Settings | rich-data, consistency, organization |
| **D4** | **No standardized page/section hierarchy.** Every screen opens with `Card title` over a low-contrast gray one-line `<Text>`; there is no PageHeader, no count/summary, no primary-action toolbar, no breadcrumb. Hierarchy is carried almost entirely by ad-hoc inline `opacity` (~250 uses across 8 values; `0.6` alone 195×) as the de-facto "secondary text" system, which means contrast drifts per screen and risks failing in light mode. | App-wide | hierarchy, readability, simplicity |
| **D5** | **IA is flat and drifted.** Sidebar (10 items, one ungrouped "Primary" group) and palette nav are two hand-maintained, already-divergent arrays. `/add` and `/decision-log` are palette-only (undiscoverable). Settings sub-sections live in non-routed `useState` (lost on refresh, not deep-linkable). Detail pages (`/content/?id=`) highlight nothing and have no breadcrumb. There is no `Wanted/Missing` route at all. | AppShell, CommandPalette, Settings, Content | organization, clarity, consistency |

**Secondary systemic gap:** **zero responsive layer** — the 24ch sidebar never collapses, interactive `Table` has no horizontal-scroll wrapper, and `global.css` has no `@media` rules — so the whole ch-grid simply overflows below ~600px. And **focus management is absent in overlays** (palette + ModalProvider don't trap or restore focus), which is the highest-severity a11y failure.

The throughline: **stop hand-rolling, lean on the components SRCL already ships, and let color/severity tokens do the hierarchy work.** Almost every fix below is *composition*, not new design.

---

## 2. North-Star Design Principles

Within SRCL's intentional terminal/monospace/dense aesthetic:

1. **Terminal ≠ monochrome.** The aesthetic permits — and needs — a disciplined color taxonomy. Use the ANSI ramp for *meaning only*: `--ansi-9-red` = error/failed/missing-available, `--ansi-2-green`/`--ansi-10-lime` = ok/downloaded/grab, `--ansi-11-yellow` = warning/cutoff-unmet, `--ansi-12-blue` = downloading/queued, gray = neutral/unmonitored. Everything else stays mono.
2. **Compose, never hand-roll.** If a surface looks like a list, it's a `Table`/`SimpleTable`. If it's a panel, it's a `Card`/`CardDouble`. If it's a row action, it's a `DropdownMenu`. No raw `border:1px solid var(--theme-*)` divs, no `<pre>`/`<ul>` data surfaces.
3. **Answer the two glance questions everywhere.** A media manager exists to tell you *"is everything healthy?"* and *"do I have room / what's missing?"* Promote disk capacity, health severity, ETA/timeleft, and missing/wanted to the top of every scan path.
4. **Rich data is the point.** Surface the fields the v3 API already returns (protocol, indexer, download client, quality profile, release status, episode/season progress, custom-format score). Use `Badge` for categoricals, `BarProgress` for progress, `Tooltip`/`Popover` for on-demand detail so rows stay one line.
5. **One primary action per screen.** A visually dominant `ActionBar`/`ActionButton` CTA per page; secondary/overflow actions in `DropdownMenu`. Single-item actions never require the bulk bar.
6. **Hierarchy via structure + weight + dividers, not opacity.** One secondary-text token, `fontWeight:600` reserved for the single primary label per group, `Divider` (single/double/gradient) to separate dense sections.
7. **Theme/tint correctness is non-negotiable.** Every new surface references real `--theme-*`/`--ansi-*` tokens (no hex, no undefined tokens like `--theme-error`). Light + dark + system + all 7 tints must render.
8. **Keyboard-first and accessible.** Trap/restore focus in overlays, route hotkeys through the mounted `react-hotkeys-hook` scopes, expose tables as real tables, color always paired with text/glyph.

---

## 3. System-Wide Workstreams

### WS-A — Information Architecture & Navigation

| Change | SRCL components |
|---|---|
| Collapse the two drifted nav arrays into **one shared typed route registry** (section + label + href + optional status-key) imported by both AppShell and CommandPaletteProvider. Normalize trailing slashes. Adding a screen = one edit. | *(glue only; feeds existing `ActionListItem` rows)* |
| Regroup the flat 10-item sidebar into **labelled collapsible sections** — **Library** (Library/Collections/Calendar/Add/Manual Import/**Wanted**), **Activity** (Activity·Queue/History), **System** (Logs/System/Decision log), **Settings**. Surface palette-only `/add` + `/decision-log` and a **new Wanted/Missing route**. Keep the `▸`/`⊹` active glyph + inverted active style. | `Accordion` (section groups) + `Link>ActionListItem` + `Divider` |
| **Live status badges** on nav rows: queue depth on Activity, health error/warning count on System, missing count on Wanted. | `Badge` (ANSI-tinted) inside `RowSpaceBetween` in the row |
| **Route Settings sub-sections** (`/settings/<section>`) — deep-linkable, refresh-stable, back-button correct. Add a **global `BreadCrumbs`** surface; map `/content` back to its parent Library section for active-state. | `BreadCrumbs`; reuse `isActiveRoute` with parent fallback |
| Move `AppShell` into `layout.tsx` (or a route-group layout) so chrome is genuinely global. | `SidebarLayout` + `Navigation` mounted once |
| Make the `✸` wordmark a **home link**; move palette trigger fully onto the Search `ActionButton`; demote `BUILD_SHA` to a `Tooltip` on the logo / System page. | `Link`, `ActionButton`, `HoverComponentTrigger`/`Tooltip` |
| Broaden palette search beyond movie/series titles to episodes/collections/settings sections; group results under `Divider` headers with a kind `Badge`. | existing palette `Card`+`Input`+`ActionListItem` + `Divider` + `Badge` |
| Switch detail routing `/content/?id=` → `/content/<id>` path segment. | Next dynamic route `app/content/[id]` |

### WS-B — SRCL Design-System Leverage (kill hand-rolled surfaces)

| Change | SRCL components |
|---|---|
| **Adopt `DropdownMenu`/`DropdownMenuTrigger` app-wide** for per-row overflow actions (search/refresh/edit/delete/toggle-monitor) on every media/queue/history/import/calendar row. | `DropdownMenuTrigger` + `DropdownMenu` |
| **Adopt `ActionBar`** for page-level toolbars (Search All / Refresh / RSS Sync / Mass Edit) with hotkeys. | `ActionBar` |
| Replace the dashboard **StatTile divs** and the **7+ raw `1px solid var(--theme-*)` panels** (system×2, CustomFormats, page, logs, library, content, QueueActions) with `Card`/`CardDouble` + `Divider`. | `Grid` + `Card` + `Badge`; `CardDouble` + `Divider` |
| Convert hand-rolled data surfaces (**Notifications/Integration `<ul>`**, **logs `<pre>`**, **Activity Downloads div stack**) to SRCL tables. | `SimpleTable` (read-only) / `Table` (interactive) |
| Move hover detail off native `title=` onto real components. | `HoverComponentTrigger` + `Tooltip`/`Popover` |
| Multi-line config (naming templates, release-profile terms, custom scripts) → `TextArea`. | `TextArea` |
| Use `RowEllipsis` for long release/file/path titles in dense rows. | `RowEllipsis` |
| Searchable lookup/provider pickers (`/add`, `/interactive`, provider selection) → `ComboBox`. | `ComboBox` |

### WS-C — Data Tables, Density & Rich Data

**Define ONE shared media-row table pattern** on semantic `Table`/`TableRow`/`TableColumn` (NOT `SimpleTable` — its string-only cells and hardcoded `ACTIVE/CLOSED` coloring can't carry Badges/BarProgress/actions/media-status). Standardize: dimmed header row, `RowEllipsis` title, colored status `Badge`, in-cell `BarProgress`, optional leading `Checkbox`, trailing `DropdownMenu` action column.

| Change | SRCL components |
|---|---|
| Make it the backbone of Library, Activity Downloads, History, Content Files, Manual Import, Interactive, Collections. | `Table`/`TableRow`/`TableColumn` + `Badge` + `BarProgress` + `RowEllipsis` + `Checkbox` + `DropdownMenu` |
| Promote Library's `SortHeader` pattern (`TableColumn` + `aria-sort` + caret) into a shared component; apply sortable headers + filter toolbar (verdict/status/hide-rejected/min-seeders) + simple paging to History, Activity, Interactive, Content Files. | `Table`/`TableColumn(align)` + `ActionBar` + `Select` + `Checkbox` + `Input` |
| **Right-align numeric columns** (size/score/seeders/age/counts) — nearly free in monospace, currently `align=` is used in exactly 1 file. | `TableColumn align` / `SimpleTable` per-column align |
| Column show/hide menu for dense grids (escape valve for new parity columns). | `ActionBar`/`DropdownMenu` of `Checkbox` toggles |
| Reserve `SimpleTable` strictly for static string-only grids (system Health/Tasks). | `SimpleTable` |

### WS-D — States, Feedback & Empty/Loading

| Change | SRCL components |
|---|---|
| **Per-panel pending states** (render each Card, `BlockLoader` in its body) instead of one global pre-content spinner that shows a zeroed dashboard. | `Card` + `BlockLoader`/`BarLoader` |
| **Standardize empty/loading/error** into one shared pattern: loading = `BlockLoader`+`Text`; empty = `Message`/Card with a single CTA; error = `AlertBanner`. Retire `SuccessBanner`; keep `useToast()` for transient confirms, `AlertBanner` for persistent in-panel state. | `BlockLoader`/`BarLoader` + `Message` + `AlertBanner` + `Button` |
| **Color-encode severity:** errors → `AlertBanner`, warnings → tinted `Badge`, reason on `Tooltip`. Apply to health, queue lifecycle, calendar, decision verdicts. | `AlertBanner` + `Badge` + `Tooltip` |
| **Real disabled states:** stop nulling `onClick`; use `disabled`/`aria-disabled` + `aria-describedby` reason (first-run Next, settings Save). | `Button(disabled)` + `AlertBanner`/`Text` |
| **Shared SaveBar + unsaved-changes guard** across settings (dirty tracking, one save, keyboard save, navigation-block confirm). | `ActionBar` + `Button` + `Badge` + `Dialog`/`ModalStack` |
| **Robust toasts:** errors via `role=alert`/`aria-live=assertive`, longer dwell, not auto-removed. | `ToastProvider` region (second assertive child) |

### WS-E — Typography, Color & Theming

| Change | SRCL components / tokens |
|---|---|
| **One shared status→token map** applied to every `Badge`/status cell (the keystone fix for D1). | `Badge` (`style` color from map) + `SimpleTable` status contract |
| **Retire ad-hoc opacity** (~250 uses). Two tiers max (label, hint) via `--theme-border-subdued` / `--theme-focused-foreground-subdued` instead of opacity (which washes out faster over light backgrounds). | `Text` + tokens |
| **Fix undefined/wrong tokens:** remove `--theme-error` (use `--ansi-9-red`) and `--theme-background-subdued` (use `--theme-window-background`); correct the logs `LEVEL_COLOR` names (`--ansi-8-gray`, `--ansi-6-teal`, `--ansi-11-yellow`, `--ansi-3-olive`) — DEBUG/WARN/TRACE currently lose color silently. | tokens only |
| **Standardize the type scale:** drop one-off `fontSize:2ch/3ch` and `lineHeight:1.5`; derive from `--font-size`, keep the 1.25 line box. Convert logs `<pre>` to `Table`/`CodeBlock`. | `Text`, `CodeBlock`, `Table`; `--font-size`, `--theme-line-height-base` |

### WS-F — Accessibility & Keyboard

| Change | SRCL components |
|---|---|
| **Focus trap + restore + background inert** for the command palette and **ModalProvider** (the highest-severity systemic a11y gap; makes SRCL `Dialog` genuinely modal). | palette `Card`/`Input`; `ModalProvider` + `Dialog` (glue) |
| **True combobox semantics** on the palette: `role=combobox` + `aria-expanded`/`aria-controls`/`aria-activedescendant`. | `Input` (+aria) + `role=listbox`/`option` rows |
| **Skip link** to `<main id="main" tabindex="-1">`. | `.sr-only` anchor in AppShell |
| **Fix active-nav contrast + focus ring** (active bg uses the same `--theme-focused-foreground` as the focus outline → invisible focus). Drive active style from `--theme-button`/`-text`. | `ActionListItem` styling |
| **Theme toggle:** move `aria-pressed` onto the role element; model as `RadioButtonGroup` (one-of-three). | `RadioButtonGroup`/`RadioButton` |
| **Route hotkeys through the mounted `react-hotkeys-hook` scopes** (⌘K, `/`, future ActionBar hotkeys), gating non-global scopes while overlays are open; expose a `?` keymap dialog. | `useHotkeys`/`HotkeysProvider`; `ActionBar`; `Dialog` |
| **Tables for SR semantics** + **per-row menus** as the accessible single-item path; wire `htmlFor`/`id` + `aria-describedby` on all form fields. | `Table`/`SimpleTable`; `DropdownMenu`; `Input`/`Select`/`Checkbox` |

### WS-G — Responsive / Narrow Viewport

| Change | SRCL components |
|---|---|
| **Collapse the 24ch sidebar into a `Drawer`** below a width threshold, toggled from Navigation; keep `SidebarLayout` for wide. | `Drawer` (side toggle) + `ActionListItem` + `ActionButton` |
| **Wrap every interactive `Table` in an `overflow-x:auto` scroll container** (or migrate to `SimpleTable`, which already ships `.scrollWrapper`). | `SimpleTable` / `Table` in wrapper |
| **Column prioritization on narrow widths:** show Title (`RowEllipsis`) + status `Badge`; move dropped columns into an `Accordion`/`DropdownMenu` detail. | `RowEllipsis` + `Badge` + `Accordion`/`DropdownMenu` |
| **Cap modals** at `max-width:min(56ch,100vw)` with `max-height` + internal scroll. | `Dialog` + `ModalStack` |
| Slim Navigation on narrow widths (drop tagline + build badge, collapse controls to overflow). | `ActionBar`/`DropdownMenu` |
| Single `useViewportWidth` hook driving Drawer-vs-SidebarLayout and column counts centrally. | *(glue only)* |

---

## 4. Per-Screen Redesign Specs

> Format per area: **Keep / Fix / Add (richer data) / Compose**. Every area in the evaluation is covered.

### 4.1 Dashboard / Home (`/`)
- **Keep:** fan-fetch with per-panel graceful degradation; pure tested helpers (`dashboard.ts`); deep-link panels; year-0001 guard.
- **Fix:** replace hand-rolled StatTile divs (drop `1px solid var(--theme-border)`, `fontSize:2ch`, undefined `--theme-error`); split severity (errors → `AlertBanner`, warnings → tinted `Badge`); per-panel `BlockLoader` (no zeroed-dashboard flash); split page into **System status** band and **Library activity** band via `Divider`; accessible glyph+`Badge` state; filtered deep-links (`?monitored=1&missing=1`).
- **Add:** **real Disk-space card** (per-root Free/Total/Used `SimpleTable` + `BarProgress`, `AlertBanner` on >90%); indexer/download-client health counts; movies-vs-series split; quality `Badge`s on rows; a Missing/Wanted rail; page-level quick actions (Search All/Refresh/RSS Sync); first-run card when `total===0`.
- **Compose:** `Grid` + `Card` + `Badge` + `RowSpaceBetween` + `SimpleTable` + `BarProgress` + `AlertBanner` + `ActionBar` + `DropdownMenu` + `BlockLoader` + `RowEllipsis` + `Divider`.

### 4.2 Library grid/list (`/library`)
- **Keep:** URL-driven `?lib=` switcher; `SortHeader` sortable columns; title/status/type filters; bulk bar; poster thumb concept.
- **Fix:** convert bulk-delete from hand-rolled overlay to SRCL `Dialog`/`ModalStack`; controlled `Checkbox` (drop the key-churn remount); remove the always-`—` TV Quality column; reduce status-cell noise (glyph + two badges); breadcrumb anchor.
- **Add:** parity columns — Quality Profile, Status (released/announced/continuing/ended), Added date, Genres/Studio/Runtime (movies); **TV episode-progress cell** (`18/24` `BarProgress`) + Seasons/Network/Status; **per-row `DropdownMenu`** (Search/Refresh/Edit/Toggle-monitor/Delete); **state-breakdown footer** (downloaded+monitored / missing-monitored / unmonitored / queued / total size); faceted filter `Drawer` (quality/missing/tags/year); view persistence in URL.
- **Compose:** `Table`/`TableRow`/`TableColumn` + `Badge` + `BarProgress` + `RowEllipsis` + `DropdownMenu` + `Dialog` + `Checkbox` + `Drawer` + `Select` + `BreadCrumbs` + `Grid` (footer).

### 4.3 Content detail (`/content`)
- **Keep:** `BreadCrumbs`; `CardDouble` header; poster with load/404 placeholder; monitor/type/tag mutations + toasts; `Files` table; series-type `Select`; `TagInput`.
- **Fix:** **collapse the three overlapping surfaces** (Structure `TreeView` + Monitoring card + duplicate Files badge row) into one per-season `Accordion`; convert heavy per-row monitor `Button`s to compact `Checkbox`; relative/localized air dates; real loading (`BlockLoader`) + `AlertBanner` errors; normalize typography off ad-hoc opacity; split the single status badge into discrete release/monitored/file-state badges.
- **Add:** per-season + per-episode **search**; download/queue state (`BarProgress` when grabbing); episode-row parity columns (size/runtime/language/release-group/custom-format chips+score/media-info via `Popover`); season progress summaries (X/Y · N files · size); Files parity columns (language/release-group/CF chips/media-info/full path `RowEllipsis`/added) + per-file `DropdownMenu` (delete/rename/manage); extended metadata (genres/studio/cert/ratings/release dates/collection/external links); page-level `ActionBar` (Search/Interactive/Refresh+Scan/Manage Files/Edit/Delete/History).
- **Compose:** `Accordion` + `Table` + `Checkbox` + `Badge` + `BarProgress` + `ActionButton` + `ActionBar` + `DropdownMenu` + `Dialog` + `HoverComponentTrigger`/`Popover` + `RowEllipsis` + `Grid` + `BlockLoader` + `AlertBanner`.

### 4.4 Add + Interactive release search (`/add`, `/interactive`)
- **Keep:** debounced lookup with ranked MOVIES/TV sections; add `Dialog` (library/profile/root/monitor/series-type/search-on-add); release `HoverComponentTrigger` score/rejection hovers; idle/loading/error/empty/ready phases.
- **Fix:** **replace `/interactive` raw "Content id" input with a title picker** (reuse `/add` lookup or `ComboBox`); fix the mislabeled "Popularity" column (split Year/Rating/Runtime); move both screens to a shared search shell; truncate long titles (`RowEllipsis`); gate `Grab` behind a `Dialog` confirm when rejected/not-allowed; surface grab-failure reason; aria-labels on numeric/icon columns.
- **Add (Interactive):** parity columns — Source/Protocol, **Age** (relative + `Tooltip` full date), Indexer, seeders/**leechers**, Language, Flags, itemized rejections (`Popover` bulleted), info-URL link; **column sort** + **filter toolbar** (hide-rejected/protocol/indexer/min-seeders); override-and-grab `Dialog`. **Add (`/add`):** genres/cert/runtime/language/studio/split ratings as `Badge`s; external links + full overview on hover; "already in library" `Badge` + `BarProgress`; optional poster wall (`Grid`+`Card`); `BreadCrumbs`/back on `/interactive`.
- **Compose:** `SimpleTable`/`Table` + `RowEllipsis` + `Badge` + `HoverComponentTrigger`/`Tooltip`/`Popover` + `ActionBar` + `Select` + `Checkbox` + `Drawer` + `ComboBox` + `BreadCrumbs` + `Dialog` + `BarProgress` + `Grid` + `Card`.

### 4.5 Activity queue + History (`/activity`, `/history`)
- **Keep:** SSE-driven live overlay; stream-state badge; self-heal `Table`; scheduled-tasks `Table` + "Run now"; History global feed + node timeline; decision-log `why·run` link.
- **Fix:** **rebuild Downloads as a real `Table`** (currently a `<div>` stack while Self-heal beside it is a Table); fold three per-row `QueueActions` buttons into one `DropdownMenu`; replace hand-rolled `QueueActions` modal + manual-import picker with `Dialog`/`ModalStack` + `Table` selection; split the single Activity card into per-section `Card`/`Accordion` with own loading/empty; honest "~estimated" label on derived Last-run; tint stream-state badge + `AlertBanner` on disconnect/failed; `RowEllipsis` on titles/paths.
- **Add:** Downloads columns — Quality/Language/Protocol `Badge`, Size+sizeleft, Indexer, Download client, in-cell `BarProgress`, **ETA/timeleft** (the single most-watched fact, absent today), differentiated status `Badge` (queued/paused/delay/importing/import-blocked/client-unavailable/failed) + hover reason; History toolbar (event-type `Select` filter, sortable headers reflecting API `sortKey`, paging), human title links instead of `shortId` buttons, quality/protocol columns, per-row `DropdownMenu` (Mark-as-failed/open content/decision log); re-grab action.
- **Compose:** `Table`/`TableRow`/`TableColumn` + `RowEllipsis` + `Badge` + `BarProgress` + `HoverComponentTrigger`/`Tooltip` + `DropdownMenu` + `Dialog`/`ModalStack` + `Card`/`CardDouble` + `Accordion` + `ActionBar` + `Select` + `AlertBanner`.

### 4.6 Calendar + Collections (`/calendar`, `/collections`)
- **Keep (Calendar):** forward-window agenda; window switcher; per-day cards; deep-links. **Keep (Collections):** title filter; optimistic monitor toggle with rollback + toasts; A–Z sort.
- **Fix (Calendar):** **status taxonomy + Legend** (downloaded / monitored-missing-available / missing-unavailable / unmonitored / downloading / cutoff-unmet) via tinted `Badge`s; window switcher → `RadioButtonGroup`; collapse sparse days (`Accordion`); single `Divider` not gradient; de-dup glyph vs badge. **Fix (Collections):** `AlertBanner` errors + `BlockLoader` loading (match Calendar); drop the redundant `Checkbox`+`Badge` duplication in the monitor cell.
- **Add (Calendar):** **real month/week grid + Today/prev/next** (`DatePicker`); episode code (S01E02)/release-type/air-time/genres/cert via `Badge`+`Tooltip`+`RowEllipsis`; queue progress (`BarProgress`); per-row `DropdownMenu` (search/toggle-monitor). **Add (Collections):** richer table (owned·missing breakdown / quality profile / min-availability / root) + completeness `BarProgress`; expandable member list (`Accordion`) with add-from-collection `Dialog`; sort/filter + bulk-edit `ActionBar`; per-row `DropdownMenu`.
- **Compose:** `DatePicker` + `ActionBar` + `RadioButtonGroup` + `Badge` + `RowEllipsis` + `HoverComponentTrigger`/`Tooltip` + `BarProgress` + `DropdownMenu` + `Accordion` + `SimpleTable`/`Grid`+`Card` + `Dialog` + `Select` + `Checkbox` + `AlertBanner` + `BlockLoader` + `Divider`.

### 4.7 Decision-log + Logs (`/decision-log`, `/logs`)
- **Keep (Decision-log):** run summary verdict-count `Badge`s; per-record `Accordion`; `KeyValueTable`/`ScoreTable`/`UpgradeComparison`; parse-confidence badges. **Keep (Logs):** file list table; Level/Lines/Refresh toolbar; newest auto-select.
- **Fix:** **in-run filter (verdict `Select`) + title search**; **recent-runs list on idle** (no UUID paste); sidebar entry for decision-log; shorten run header to `shortId` + full on hover; **demote per-record Raw JSON + coords behind a nested `Accordion`**; color-encode verdicts (grab=lime/reject=red/upgrade=accent); align records into columns; grab/reject get real comparison tables not prose; **re-platform logs `<pre>` to `SimpleTable`** (Time/Level/Logger/Message, status coloring, `RowEllipsis`); fix broken `LEVEL_COLOR` token names; re-theme the inline-red `AlertBanner`.
- **Add:** clickable verdict-count filters; per-row decision actions (copy-run-id/jump-to-content/grabbed-release link); run-level metadata (start/end/duration/candidate count); logs per-row expand (full message + exception `Dialog`/`Accordion`), Clear-logs + last-refreshed `Badge` + size column; `Tooltip` on confidence explaining the metric.
- **Compose:** `Select` + `Input` + `Badge` + `Table`/`SimpleTable` + `RowEllipsis` + `Accordion` + `HoverComponentTrigger`/`Tooltip` + `Dialog`/`ModalStack` + `ButtonGroup` + `ActionBar` + `Navigation`/`ActionListItem` (sidebar) + `Divider`.

### 4.8 Manual Import (`/import`)
- **Keep:** scan-then-review phasing; per-row include `Checkbox`; `TargetPicker` lookup+seed fallback; rejection surfacing concept; pre-check defaults.
- **Fix:** **show the resolved human match label** (Title (Year) · SxxEyy) not `#hash` for pre-included rows; controlled include `Checkbox` (drop `defaultChecked`); rejection reasons → `Popover` list (keyboard/SR-reachable) not native `title=`; status-colored `Badge` (ok=green/rejected=red); move match action out of the data cell into a per-row `DropdownMenu`; `RowEllipsis` on paths; persistent results panel (source→destinationPath) instead of ephemeral toast.
- **Add:** editable Quality/Language/Release-group/Release-type and TV Season/Episode pickers via `Dialog`; missing columns (Language chips/Release group/CF score); **bulk select-all + bulk field edit** (`ActionBar` + `DropdownMenu`); column sort + filter (rejected/unmatched/quality); **import-mode** (Move vs Copy/Hardlink) `RadioButtonGroup` + Move confirm `Dialog`; folder browser (`TreeView`/`BreadCrumbs`).
- **Compose:** `Table`/`TableRow`/`TableColumn` + `RowEllipsis` + `Badge` + `HoverComponentTrigger`/`Popover` + `DropdownMenu` + `Dialog`/`ModalStack` + `Select`/`RadioButtonGroup` + `Checkbox` + `ActionBar` + `Accordion`/`SimpleTable` (results) + `AlertBanner` + `TreeView`.

### 4.9 First-run wizard + Login (`/first-run`, `/login`)
- **Keep:** 3-state first-run card; 5-step `Dialog` wizard; schema-driven optional indexer/client; success banner + Go-to-Library; login form + `safeNext()` + Suspense.
- **Fix:** **add a "Secure this server" auth step** (method `Select` None/Basic/Forms + Username/Password/Confirm + `AlertBanner` warning on None) — the upstream first-run's actual purpose; **real disabled `Next`** with reason; **partial-failure recovery** (library-created-but-integration-failed → targeted per-entity retry, not blind re-POST); `AlertBanner`+retry on daemon-unreachable (stop swallowing network errors); footer hierarchy (primary Next, secondary Back, Skip/Cancel into overflow); replace the `<ul>` Finish review with a status-colored `SimpleTable`; re-theme login's hardcoded-red `AlertBanner`; expose `StepIndicator` to AT (un-hide + `aria-live`); associate labels via `htmlFor`/`id`; login chrome (`ThemeBarToggle` + build `Badge`).
- **Add:** per-implementation **Test connection** before save (`BlockLoader`+`AlertBanner`); collapse optional integrations into one `Accordion` gated by `Checkbox`; root-folder free-space/path-exists feedback; Remember-me `Checkbox` + recovery link + auth-method `Badge` on login; media-type-aware defaults.
- **Compose:** `Card`/`CardDouble` + `Select` + `Input` + `Checkbox` + `AlertBanner` + `Button(disabled)` + `BlockLoader` + `SimpleTable` + `Accordion` + `RadioButtonGroup` + `Divider` + `RowSpaceBetween` + `Badge` + `ThemeBarToggle`.

### 4.10 Settings hub + shared patterns
- **Keep:** `useAsync` loader; `ManagedBadge`; `useToast`; per-section components.
- **Fix:** **grouped + routed IA** — cluster 15 tabs into labelled groups (Profiles & Formats / Indexers & Clients / Media Management / System) via `Accordion` or an in-page `ActionListItem` rail, each with a one-line summary `Text`; back every section with `/settings/<section>` + `BreadCrumbs`; **shared SaveBar** (dirty/pending/keyboard save) replacing per-card buttons; **unsaved-changes guard** `Dialog`; replace hand-rolled `ConfirmDialog` with SRCL `ModalStack` (focus-trap, accurate consequence copy as a prop); standardize feedback on `useToast` + `AlertBanner` (retire `SuccessBanner`, re-theme `ErrorBanner`); real `Checkbox` toggles; basic/advanced disclosure.
- **Add:** section search/filter; provider/list row enrichment (Test/Edit/Enable-Disable/Delete `DropdownMenu`, capacity `BarProgress`, status `Badge`).
- **Compose:** `Accordion`/`ActionListItem` + `Text` + `Divider` + `BreadCrumbs` + `ActionBar` + `Button` + `Dialog`/`ModalStack` + `AlertBanner` + `Checkbox` + `Select` + `DropdownMenu` + `SimpleTable`/`Table` + `Badge` + `BarProgress`.

### 4.11 Settings: Profiles (Quality Profiles / Definitions / Custom Formats / Release / Delay)
- **Keep:** live CF test box; per-quality allow/cutoff logic; managed locking; delete `ConfirmDialog`; toasts.
- **Fix:** **replace Quality Profiles single-`Select` editor with a `SimpleTable` overview** (Name + ordered quality `Badge` chips + Cutoff + Upgrades + Min CF score + actions) → `Dialog` editor (unify all five cards' interaction model); **render terms/specs as inline `Badge` chips** on Release Profile + Custom Format rows (not `2R·1I·3P` / `N specs`); group long editors into titled `CardDouble` sub-panels; split Delay Profiles' single Enabled into Usenet/Torrent `Checkbox`es; disambiguate ✗-remove vs ✗-delete glyphs; replace `<ul>`/inline-styled spec boxes with SRCL rows.
- **Add:** **field-level help** (`HoverComponentTrigger`/`Tooltip` on `?`); missing QP fields (cutoffFormatScore, minUpgradeFormatScore, Language `Select`, **per-profile CF-score `SimpleTable`**); Quality Definitions grouped by source (`Accordion`) + human runtime readout; Clone + Import/Export CF (`Dialog`+`TextArea`/`CodeBlock`); "used by N" on delete; per-row `DropdownMenu`.
- **Compose:** `SimpleTable` + `Badge` + `Dialog`/`ModalTrigger` + `RowSpaceBetween` + `RowEllipsis` + `HoverComponentTrigger`/`Tooltip` + `CardDouble` + `Input`/`Select` + `Accordion` + `DropdownMenu` + `TextArea` + `CodeBlock` + `Checkbox`.

### 4.12 Settings: Connections (Root Folders / Import Lists / Notifications / Remote Path / Integration)
- **Keep:** schema-driven credential fields; password privacy; managed detection; import-list exclusions; per-row Sync/Test.
- **Fix:** **convert hand-rolled Notifications + Integration `<ul>`s to `Table`** (unify with the table-based Root Folders/Import Lists/Remote Path); **status/capabilities as discrete colored `Badge`s** (per-event chips, enabled/protocol/priority, auto-add/refresh, accessible/unavailable); move inline edit into a **`Dialog`**; fold row actions into `DropdownMenu`; mark required fields accessibly via schema (not name-heuristic); raise label/help contrast; split overloaded Import Lists card (`Accordion`).
- **Add:** **two-step provider picker** (`Dialog` + `Grid` of provider `Card`s w/ descriptions/info links, `ComboBox` filter); full notification event set gated on `supportsOn*`; Root Folders Total/percent-used `BarProgress` + Unmapped count + browse link; **persisted per-row health/last-test `Badge`** + section-top health `AlertBanner`.
- **Compose:** `Table`/`TableRow`/`TableColumn`/`SimpleTable` + `Badge` + `RowSpaceBetween` + `DropdownMenu` + `Dialog`/`ModalTrigger` + `Grid` + `Card`/`CardDouble` + `ComboBox` + `HoverComponentTrigger`/`Tooltip` + `BarProgress` + `AlertBanner` + `Checkbox` + `Accordion` + `Divider`.

### 4.13 Settings: Misc (Naming / Tags / Security / SystemBackup)
- **Keep:** live naming preview (debounced); permission/extra-files cards; tag CRUD + `ConfirmDialog`; auth-method `Select` + revoke warning; backup `Table` + Restore/Delete confirms.
- **Fix:** **group the flat naming token palette into category `Accordion`/`CardDouble`** (Movie/Quality/MediaInfo/Edition/CustomFormat) with `HoverComponentTrigger`/`Tooltip` examples (off native `title=`); unify booleans on real `Checkbox`; render Tags as a `Grid` of `Card`s or a `Table` (drop raw id to tooltip); backup Type as colored `Badge` + relative-age `Tooltip` + path column + actions into `DropdownMenu`; raise label contrast; validation via `AlertBanner`/glyph not color-only; shared SaveBar across the multiple save buttons; elevate destructive zones (`CardDouble`/DANGER-tinted).
- **Add:** naming Rename/Replace-illegal `Checkbox` + Colon-replacement `Select`; **per-tag usage counts** (`Badge` per entity type, unused flag, linked-items `Dialog`); API-key card (read-only + copy + reset `Dialog`) + Auth-required `Select`; backup scheduling/retention card.
- **Compose:** `Accordion` + `Button`(chips) + `HoverComponentTrigger`/`Tooltip` + `CardDouble` + `Checkbox` + `Select` + `Grid` + `Card` + `Badge` + `Dialog`/`ModalStack` + `SimpleTable`/`Table` + `RowEllipsis` + `DropdownMenu` + `AlertBanner` + `Input` + `ActionBar`.

### 4.14 Cross-cutting visual shell (PageHeader)
- **Add the single reusable PageHeader** every screen consumes: `CardDouble` title bar + count/summary `Badge` + right-aligned `ActionBar` (dominant primary `ActionButton`, secondary in `DropdownMenu`) + `BreadCrumbs` on detail routes — replacing the `Card title + gray Text` opener everywhere.
- **Compose:** `CardDouble` + `Text` + `Badge` + `ActionBar` + `ActionButton` + `DropdownMenu` + `BreadCrumbs`.

---

## 5. Phased Roadmap

| Phase | Theme | Scope | Rough effort |
|---|---|---|---|
| **Phase 0 — Foundations** | The keystones everything hangs off. | (1) **Status→token color map** (WS-E) + ANSI-tint `Badge` helper; fix undefined/wrong tokens (`--theme-error`, logs `LEVEL_COLOR`, `--theme-background-subdued`). (2) **Shared route registry** (WS-A) ending nav drift. (3) **Shared media-row `Table` pattern** + scroll-wrapper rule (WS-C/G). (4) **Reusable PageHeader** (§4.14). (5) **Overlay focus trap/restore** in palette + ModalProvider (WS-F). (6) Secondary-text token to retire opacity (WS-E). | **L** (highest-leverage; unblocks all per-screen work) |
| **Phase 1 — IA & shell** | Make the app navigable and global. | Grouped collapsible sidebar + Wanted route + surface `/add`,`/decision-log` (WS-A); routed Settings + global `BreadCrumbs`; AppShell into `layout.tsx`; `/content/<id>` path route; live nav status badges; home-link logo; skip link + active-nav contrast fix. | **M–L** |
| **Phase 2 — Operational surfaces** | The screens that answer "healthy? room?". | Dashboard disk card + severity split + per-panel loaders (§4.1); **Activity Downloads `Table` + ETA/timeleft + status taxonomy** (§4.5); Decision-log filter/search/recent-runs + logs `SimpleTable` (§4.7). | **L** |
| **Phase 3 — Library & content depth** | Rich-data parity. | Library parity columns + per-row `DropdownMenu` + footer + filter `Drawer` (§4.2); Content per-season `Accordion` + Files table + per-episode search (§4.3); History toolbar/filter/sort (§4.5). | **L** |
| **Phase 4 — Acquisition & import** | Decision-grade flows. | Interactive title-picker + full release table + sort/filter (§4.4); `/add` enrichment; Manual Import editable fields + bulk + match-label fix + results panel (§4.8); Calendar month grid (`DatePicker`) + status taxonomy (§4.6). | **L** |
| **Phase 5 — Settings & onboarding** | Config trustworthiness. | Settings grouped IA + SaveBar + unsaved guard + `ModalStack` confirm (§4.10); Profiles overview tables + chips + field help (§4.11); Connections table unification + provider picker + health (§4.12); Misc naming palette + tag usage + security/backup (§4.13); First-run auth step + partial-failure recovery (§4.9). | **L** |
| **Phase 6 — Responsive & polish** | Narrow viewport + a11y completeness + states. | Sidebar `Drawer` collapse + column prioritization + capped modals (WS-G); combobox semantics + hotkey scopes + theme `RadioButtonGroup` + form labelling (WS-F); standardized empty/loading/error + robust toasts (WS-D); demote build-SHA, slim top bar. | **M** |

---

## 6. Quick Wins vs Big Bets

### Quick wins (high-impact / low-effort)
- **Status→color `Badge` map** — one helper, applied broadly; single biggest perceived-quality jump (fixes D1).
- **Fix broken/undefined tokens** (`--theme-error`, logs `LEVEL_COLOR` names, `--theme-background-subdued`) — currently failing/falling back silently.
- **Right-align numeric columns** — nearly free in monospace; instant scannability.
- **Wrap interactive `Table`s in `overflow-x:auto`** (or move to `SimpleTable`) — stops page-breaking overflow.
- **Shared route registry** — ends nav drift, surfaces `/add`/`/decision-log` in one edit.
- **Inline `Badge` chips for Release Profile / Custom Format rows** (replace `2R·1I·3P`/`N specs`).
- **Field-level help via `Tooltip`** on Profiles/Connections labels.
- **Real disabled states** (drop `onClick:undefined`) + **Settings AlertBanner/BlockLoader consistency** (Collections to match Calendar).
- **Show the resolved match label** (not `#hash`) on Manual Import pre-included rows.
- **Skip link** + **active-nav contrast/focus fix**.
- **Per-tag usage `Badge`s** and **backup Type colored `Badge` + relative age**.

### Big bets (high-impact / higher-effort)
- **Shared media-row `Table` pattern** as the backbone of every list (kills D3; carries all rich-data).
- **`DropdownMenu` + `ActionBar` adopted app-wide** for per-row + page-level actions (kills D2's affordance gap, biggest keyboard/a11y win).
- **Routed Settings IA + grouped sidebar + global breadcrumbs** (kills D5).
- **Activity Downloads rebuild with ETA/timeleft + status taxonomy** (the most load-bearing operational fact, absent today).
- **Calendar month grid via `DatePicker`** (the route doesn't behave like a calendar today).
- **Content per-season `Accordion` consolidation + Files table** (fixes the worst duplication/organization/rich-data at once).
- **Overlay focus management** (palette + ModalProvider) — highest-severity a11y, spans every modal.
- **Responsive sidebar `Drawer` + column prioritization** (app is unusable below ~600px today).

---

**Bottom line:** the fastest path to a UI that reads as a serious Radarr/Sonarr peer is Phase 0 — ship the status-color map, the shared route registry, the one media-row `Table` pattern, the reusable PageHeader, and overlay focus management. Those five foundations convert cellarr's solid-but-underused SRCL baseline into a dense, colored, keyboard-driven, theme-correct surface, and every per-screen rich-data fix then composes cleanly on top of them — without a single new primitive.
---

## Implementation progress (live)

Shipped + gated (typecheck + SRCL-lint + 363 web tests) + deployed + verified on rinzler:

- **Phase 0 colour taxonomy (D1):** `web/app/_lib/status.ts` + `StatusBadge`; applied to Library, Dashboard health, Activity lifecycle, Interactive + Manual-Import rejected/ok. Fixed silently-broken tokens (logs `LEVEL_COLOR`, dashboard `--theme-error`, logs bg). — `f96d24d`, `a5fd695`, `e752c85`
- **Density:** Quality-profile cutoff 16-row radio → `Select`; Calendar range → horizontal `ButtonGroup`. — `f4bfa90`, `62bf567`
- **Select stale-label fix:** profile + cutoff Selects key on resolved value (the v3 API ships `rank-N` placeholders resolved async). — `dcab488`
- **Clarity:** content-detail raw `#id` "Structure" hidden for movies. — `3d432ae`
- **Organization:** Settings 15-tab wall → 4 labelled bands (Profiles/Connections/Media/General). — `0ba191e`
- **Logs:** in-log substring Filter input. — `cf794d8`
- **Responsive:** AppShell collapses the sidebar behind a hamburger below 768px (was desktop-only). — `e635375`
- **Content-detail enrichment:** coloured status + Path + TMDB link (`tmdbId` added to `DetailView`). — `7f49d70`, `d76ceb6`
- **Broadened colour:** History event chips + Activity self-heal blocklist chip; System health table coloured (carries per-row tone). — `d76ceb6`, `deaee1e`
- **Table overflow:** every SRCL `Table` wrapped in an `overflow-x:auto` container (one fix, app-wide). — `b261464`
- **Activity ETA:** download `timeleft` surfaced next to status. — `13edd63`
- **Overlay a11y:** command palette restores focus to its trigger on close + traps Tab within the dialog. — `3e125bd`

**UI facelift punch-list: COMPLETE.** The only remaining item is a backend feature, not a UI change:

- **Content-detail rich metadata (poster / overview / genres / runtime / ratings)** — requires a backend TMDB-enrichment pass: the v3 API returns `overview:''`/`runtime:0`/no images/genres because cellarr stores only catalogue identity. Needs new fields in the content/movie model + an enrichment job in `cellarr-meta` + the v3 projection to populate them; the existing UI ("No poster"/"No overview" placeholders) then renders them. Scope is a discrete feature on the order of the Cardigann engine, tracked separately.

Possible future polish (not blocking): Library parity columns + per-row `DropdownMenu`; routed Settings sub-sections + breadcrumbs; per-screen density passes.
