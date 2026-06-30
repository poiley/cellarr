# Per-page UX review (MCP-grounded) — refinement pass

Audited live (cellarr.rinzler.cloud, build C01E01B, desktop dark) every page. The
earlier facelift addressed colour taxonomy, density, settings grouping, responsive,
and table scroll. This pass refines **information hierarchy, label↔value association,
and tabular scannability** within the SRCL terminal aesthetic. SRCL-only; no new
primitives; light+dark+system must hold; the web gate (typecheck + srcl-lint + 363
vitest) must stay green.

## Systemic
- **Tabular numerics:** right-align + `font-variant-numeric: tabular-nums` on numeric
  data columns (Library Year/Size; any size/count column) so digits line up and scan.
- **Status-chip consistency:** colour the remaining status chips via `StatusBadge`
  (Activity scheduled-task "QUEUED" last-status).

## Per page
- **Dashboard** (`web/app/page.tsx`):
  - Remove the redundant page `<h2>cellarr</h2>` (the sidebar logo already says it);
    it duplicates and adds no information.
  - Give the Overview stat-tile **values** clear size/weight hierarchy (the number is
    the data; the label is secondary) — larger/bolder value, quiet label.
  - Place "Downloading now" + "Recent activity" **side by side** (two columns on wide
    viewports, stacking on narrow) to use the horizontal space and cut dead vertical
    space. Keep each card's content + "View …" link.
- **System** (`web/app/system/page.tsx`):
  - Health table: the Status sits pinned far-right with a wide empty gap from the
    Check name — **constrain the table width** (or tighten columns) so each check and
    its status read as one unit.
  - The "All health checks passed" success banner is oversized for one line — make it
    **compact** (a single tinted line, not a tall full-width block) when there are no
    warnings.
- **Activity** (`web/app/activity/page.tsx`):
  - Scheduled-tasks **RUN NOW** buttons are isolated in a far-right column,
    disconnected from their task row — bring the action **adjacent** to the row
    (reduce the gap so action associates with task).
  - Colour the per-task **last-status** chips (QUEUED → info) via `StatusBadge`.
- **Collections** (`web/app/collections/page.tsx`):
  - Empty state explains collections come from import lists but offers no path —
    add an **actionable link** to Settings → Import Lists.

## Lower priority / inherent (not in this pass)
- Verbose intro paragraphs (History/Decision-log/Collections) — copy, leave as-is.
- Interactive standalone raw-UUID input — works via deep-link from items.
- Dead space on sparse pages — a test-data artifact (real libraries fill the views).
- The boxed-fieldset framing is the SRCL aesthetic — kept intentionally.
