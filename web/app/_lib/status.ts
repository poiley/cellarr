// Shared status → severity-tone map (facelift WS-E / D1). The app was effectively
// monochrome: every status rendered as an identical gray chip even though SRCL
// ships theme-aware ANSI tokens. This is the one place that maps a status token to
// a semantic colour, so colour carries meaning consistently everywhere. Colour is
// always PAIRED with the label text (and often a glyph) — never the sole signal.

export type Tone = 'ok' | 'warn' | 'error' | 'info' | 'neutral';

/** SRCL ANSI tokens, used for meaning only (severity/state). Stable across themes. */
export const TONE_COLOR: Record<Tone, string> = {
  ok: 'var(--ansi-2-green)',
  warn: 'var(--ansi-11-yellow)',
  error: 'var(--ansi-9-red)',
  info: 'var(--ansi-12-blue)',
  neutral: 'var(--ansi-8-gray)',
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
  PENDING: 'info',
  COMPLETED: 'ok',
  GRABBED: 'ok',
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
