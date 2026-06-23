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
  `global-fonts.css` and `colors.json`-derived tokens so the terminal aesthetic is exact.
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
- A small end-to-end smoke set drives the real API (a release flows from search → grab → import and
  shows up correctly, including a decision-log entry).

## References
[10-ui.md](../10-ui.md), [09-api.md](../09-api.md), [11-testing.md](../11-testing.md).
