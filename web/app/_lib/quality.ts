// Quality-name resolution glue. The v3 quality-profile payload carries each
// quality's id under `items[].quality.id`, but the embedded `quality.name` is an
// unreliable placeholder — the seeded daemon serves "rank-20"/"rank-21" there.
// The authoritative human names ("WEBDL-1080p", "Bluray-1080p", …) live in
// GET /api/v3/qualitydefinition, keyed by the same `quality.id`. This module
// builds that id -> name lookup so the profile editor can render real names in
// the qualities list and cutoff selector.
//
// Pure data/routing glue (no UI primitive), so it does not violate SRCL-only.

import type { QualityDefinition } from '@lib/api/types';

/** Maps a quality id (as a string, matching the form's item ids) to a name. */
export type QualityNameMap = Map<string, string>;

/**
 * Build an id -> name map from the quality definitions. Definition names that
 * look like the unhelpful "rank-N" placeholder are skipped so they never shadow
 * a better name; if every definition is a placeholder the map is still keyed and
 * callers fall back to the raw value.
 */
export function buildQualityNameMap(
  defs: QualityDefinition[] | undefined
): QualityNameMap {
  const map: QualityNameMap = new Map();
  if (!Array.isArray(defs)) return map;
  for (const d of defs) {
    const q = d?.quality;
    if (!q || typeof q.id !== 'number') continue;
    const name = typeof q.name === 'string' ? q.name : undefined;
    const title = typeof d.title === 'string' ? d.title : undefined;
    // Prefer the quality name, fall back to the definition title; skip the
    // "rank-N" placeholder so a real name elsewhere can win.
    const resolved =
      name && !isPlaceholderName(name) ? name : title && !isPlaceholderName(title) ? title : undefined;
    if (resolved) map.set(String(q.id), resolved);
  }
  return map;
}

/** True for the "rank-20"-style placeholder the daemon emits when it has no name. */
export function isPlaceholderName(name: string): boolean {
  return /^rank-\d+$/i.test(name.trim());
}

/**
 * Resolve a display name for a quality id, preferring the definition map but
 * falling back to the (possibly placeholder) name carried on the profile item,
 * then to a generic label.
 */
export function resolveQualityName(
  id: string,
  fallbackName: string | undefined,
  map: QualityNameMap
): string {
  const fromDefs = map.get(id);
  if (fromDefs) return fromDefs;
  if (fallbackName && !isPlaceholderName(fallbackName)) return fallbackName;
  // Last resort: surface the id rather than the bare "rank-N" placeholder.
  return `Quality ${id}`;
}
