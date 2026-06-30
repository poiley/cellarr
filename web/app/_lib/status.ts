// Shared status → severity-tone map (facelift WS-E / D1). The app was effectively
// monochrome: every status rendered as an identical gray chip even though SRCL
// ships theme-aware ANSI tokens. This is the one place that maps a status token to
// a semantic colour, so colour carries meaning consistently everywhere. Colour is
// always PAIRED with the label text (and often a glyph) — never the sole signal.

export type Tone = 'ok' | 'warn' | 'error' | 'info' | 'neutral';

// Theme-aware semantic tones. The raw --ansi-* primitives are fixed sRGB and only
// contrast with one background (pure blue is illegible on black, pure yellow on
// white), so these resolve to the per-theme --tone-* tokens defined in global.css
// (an OKLCH projection of the same palette colour, brightened on dark / darkened on
// light). Colour is always paired with the label text — never the sole signal.
export const TONE_COLOR: Record<Tone, string> = {
  ok: 'var(--tone-ok)',
  warn: 'var(--tone-warn)',
  error: 'var(--tone-error)',
  info: 'var(--tone-info)',
  neutral: 'var(--tone-neutral)',
};

// Status tokens that recur across screens (library, activity, history, health,
// queue, decisions). Anything unmapped is neutral.
const TONE_BY_STATUS: Record<string, Tone> = {
  // file / library
  DOWNLOADED: 'ok',
  IMPORTED: 'ok',
  MISSING: 'warn',
  MONITORED: 'info',
  UNMONITORED: 'neutral',
  // health
  OK: 'ok',
  HEALTHY: 'ok',
  WARNING: 'warn',
  WARN: 'warn',
  ERROR: 'error',
  // queue / download lifecycle
  DOWNLOADING: 'info',
  QUEUED: 'info',
  SCHEDULED: 'info',
  PENDING: 'info',
  IMPORTING: 'info',
  COMPLETED: 'ok',
  GRABBED: 'ok',
  SENT: 'ok',
  FAILED: 'error',
  STALLED: 'error',
  BLOCKLISTED: 'error',
  // decision verdicts
  ACCEPTED: 'ok',
  REJECTED: 'error',
  RELEASED: 'neutral',
};

/** The severity tone for a status token (case-insensitive); neutral if unknown. */
export function toneFor(status: string | null | undefined): Tone {
  if (!status) return 'neutral';
  return TONE_BY_STATUS[status.trim().toUpperCase()] ?? 'neutral';
}

/** Convenience: the colour token for a status. */
export function statusColor(status: string | null | undefined): string {
  return TONE_COLOR[toneFor(status)];
}
