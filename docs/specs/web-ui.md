# Spec: web/ (the frontend)

## Responsibility
The cellarr web UI: a Next.js app composed **exclusively** from Sacred/SRCL components, talking to
`cellarr-api`. Built to static assets and embedded in the daemon via `rust-embed`.

## The hard rule
**Every UI primitive comes from SRCL.** No bespoke components, no second component library, no
hand-rolled design system. Compose screens from existing SRCL components; if something seems missing,
compose it from what exists. Adding a new component is a flagged decision following SRCL's own
conventions, not a unilateral act. See [../10-ui.md](../10-ui.md). SRCL is **MIT**; Next.js 16/React 19.

## Consumption policy (decide at scaffold, record here)
- Consume SRCL either as the `srcl` npm dependency **or** by vendoring `components/`, `common/`,
  `modules/`, and the global CSS following SRCL's structure. Bring over SRCL's `global.css` /
  `global-fonts.css` and `colors.json`-derived tokens so the terminal aesthetic is exact — including
  its `body.theme-light` / `body.theme-dark` (and `tint-*`) classes, which are the theming mechanism.

## Theming (required): light, dark, system default
- Support **both** SRCL themes and **default to the OS preference** with a persisted manual override
  (System / Light / Dark). Implement via SRCL's body classes — see [../10-ui.md](../10-ui.md) §Theming.
- A small **theme controller** (the only allowed non-component code): on load apply
  `theme-dark`/`theme-light` from `matchMedia('(prefers-color-scheme: dark)')`; subscribe to its
  `change` so "System" follows the OS live; persist Light/Dark overrides (localStorage); set CSS
  `color-scheme`; and set the initial class **pre-hydration** to avoid a theme flash.
- Build the toggle from existing SRCL controls (`RadioButtonGroup` or `DropdownMenu`) — no new component.
- The canonical component catalog is `reference/www-sacred/components/AGENTS.md`; raw sources at
  `https://sacred.computer/llm/components/<Name>.tsx.txt`.

## Screens (v1)
Build the screens in [../10-ui.md](../10-ui.md) §screen-mapping. The **decision-log screen** is the
signature feature (use `Accordion` + `CodeBlock` to expand any grab/reject/upgrade and show parsed
fields + CF-score breakdown + on-disk comparison).

## Behavior
- Talk to the native `/api/v1` (typed against the generated OpenAPI spec); subscribe to the WS/SSE
  push for live queue/import/decision updates rather than polling.
- Develop against a **mock API server** so UI work parallelizes with backend work.
- Accessible and keyboard-navigable consistent with SRCL's terminal aesthetic.

## Test obligations
- `tsc --noEmit` clean; vitest component tests for assembled screens (render + interaction).
- The **SRCL-only lint** passes: an allowlist of importable UI modules; CI fails if a UI primitive
  outside the SRCL set is introduced.
- **Theme tests:** the controller resolves System → the `prefers-color-scheme` body class, reacts to
  a `matchMedia` `change` while on System, honors a persisted Light/Dark override, and sets
  `color-scheme`; key screens render correctly under both `theme-light` and `theme-dark`; the initial
  class is set pre-hydration (no theme flash).
- A small end-to-end smoke set drives the real API (a release flows from search → grab → import and
  shows up correctly, including a decision-log entry).

## References
[10-ui.md](../10-ui.md), [09-api.md](../09-api.md), [11-testing.md](../11-testing.md).
