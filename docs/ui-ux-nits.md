# Granular per-page UX nits (MCP-grounded, fidelity pass)

Deep visual scrutiny of every page (build 4619769). SRCL-only; light+dark must
hold; web gate (typecheck + srcl-lint + 363 vitest) stays green. ~45 nits.

## Content detail (`web/app/content/page.tsx`) — biggest
1. **Two-column layout**: poster goes in a LEFT column (~30ch); Year/Runtime/Genres/
   Rating/Quality profile/Total size/Status/Path/TMDB + the Monitored toggle + Overview
   go in a RIGHT column (flex:1) that fills the empty space currently right of the
   poster. Stack (poster on top) below ~700px.
2. Make the poster larger in its left column (~30ch wide, 2:3 aspect).
3. Metadata rows: tighten — in the right column, label + value sit close (not a
   full-width spread); keep the label quiet, value emphasized.
4. "Total size — " when no file → "Not downloaded".
5. Monitored toggle button: inline-sized, not full-width.
6. Drop the repetitive nothing; keep the genre chips + coloured rating star.
7. The SEARCH (primary) / REFRESH / HISTORY actions: keep grouped, sit under the
   right column or full width below the two columns.

## Dashboard (`web/app/page.tsx`)
8. Relocate the orphaned "version 0.0.0" out of the top-left of the Overview card —
   into the card's title legend region or drop it (the top bar already shows the build).
9. "Recently added" rows use a leading ▸ that implies expandable but they are links —
   change to a clear link affordance (trailing → or plain link), not a disclosure ▸.

## Library (`web/app/library/page.tsx`)
10. Put the Filter text input and the status dropdown on ONE row (side by side), not
    two stacked rows.
11. Vertically center each title with its poster thumbnail (currently top-aligned).

## System (`web/app/system/page.tsx`)
12. Scheduled-tasks RUN NOW is still pinned far-right (only Activity was fixed) —
    associate the action with its row (group status+action like Activity).

## Activity (`web/app/activity/page.tsx`)
13. Self-heal table: hide the Indexer column when every value is "—".

## Settings → Quality Profiles (`web/app/settings/_components/QualityProfiles.tsx`)
14. Compact the 16-row qualities allow-list (tighter row height + smaller ▲▼ buttons).
15. Drop the repetitive "Allow " prefix on every quality row — the checkbox already
    means allow; show just the quality name.
16. Make "DELETE PROFILE" visually subordinate to "SAVE PROFILE" (smaller/secondary,
    not a full-width prominent button); SAVE is the primary action.

## Add (`web/app/add/page.tsx`)
17. Hide the "Popularity" column when no result carries popularity (all "—").

## Logs (`web/app/logs/page.tsx`)
18. Log-files list: the VIEW buttons are isolated far-right from their filenames —
    make each file row cohesive (action adjacent / row reads as a unit).

## Intros (`history`, `decision-log`, `import` page.tsx)
19. Tighten the 2–3 line explanatory intro paragraphs to one concise line each.
