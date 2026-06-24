// Activity-screen helpers (owned by the activity screen). Keeps the page
// component lean: the scheduled-task shape, the v3 task fetch/run, and the
// next-run countdown formatting live here so the JSX stays declarative.
//
// The v3 task surface (`GET /api/v3/system/task`) is not modelled in the shared
// @lib/api/types, so its shape is declared here and read via the client's
// generic `requestV3` escape hatch (documented for exactly this case).

import { api } from '@lib/api/client';

/**
 * A scheduler job as the daemon serializes it at `GET /api/v3/system/task`.
 *
 * Per the backend report: `nextExecution` (the countdown source), `interval`,
 * and `lastStatus` are REAL; `lastExecution` is DERIVED (nextExecution -
 * interval) and `lastDuration` is a hardcoded placeholder until the scheduler
 * persists a real run-completed timestamp/duration. The UI surfaces what's real
 * and labels the derived bits honestly.
 */
export interface SystemTaskV3 {
  id: string | number;
  name: string;
  /** The command name to POST to `/api/v3/command` for "Run now". */
  taskName: string;
  /** Cadence in minutes. */
  interval: number;
  /** ISO timestamp of the next scheduled run. */
  nextExecution: string;
  /** ISO timestamp of the last run (DERIVED), or null if never run. */
  lastExecution: string | null;
  /** Duration of the last run (currently a hardcoded placeholder). */
  lastDuration?: string;
  /** Outcome of the last run (REAL): e.g. "completed", "failed", "ok". */
  lastStatus?: string;
}

/** Fetch the scheduler's registered tasks. */
export function getSystemTasks(signal?: AbortSignal): Promise<SystemTaskV3[]> {
  return api.requestV3<SystemTaskV3[]>('/system/task', { signal });
}

/**
 * Trigger a task immediately by posting its command name to `/api/v3/command`.
 * Returns the accepted command resource; throws ApiError on a structured error.
 */
export function runTaskNow(taskName: string, signal?: AbortSignal) {
  return api.runCommandV3({ name: taskName }, signal);
}

/**
 * Format the time until `iso` as a compact countdown ("in 4m 12s", "due now",
 * "overdue 30s"). Returns "—" for a missing/invalid timestamp. `now` is
 * injectable for deterministic tests.
 */
export function formatCountdown(iso: string | null | undefined, now: number = Date.now()): string {
  if (!iso) return '—';
  const target = Date.parse(iso);
  if (Number.isNaN(target)) return '—';
  const deltaMs = target - now;
  const absSecs = Math.round(Math.abs(deltaMs) / 1000);
  if (absSecs < 1) return 'due now';
  const human = humanizeDuration(absSecs);
  return deltaMs >= 0 ? `in ${human}` : `overdue ${human}`;
}

/** Render a count of seconds as "1h 2m", "4m 12s", "30s". */
export function humanizeDuration(totalSecs: number): string {
  const h = Math.floor(totalSecs / 3600);
  const m = Math.floor((totalSecs % 3600) / 60);
  const s = totalSecs % 60;
  const parts: string[] = [];
  if (h > 0) parts.push(`${h}h`);
  if (m > 0) parts.push(`${m}m`);
  // Only show seconds when there's no hours component, to keep it compact.
  if (h === 0 && (s > 0 || parts.length === 0)) parts.push(`${s}s`);
  return parts.join(' ');
}

/** Format an ISO timestamp for the last-run column ("—" when null/invalid). */
export function formatIso(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? '—' : d.toLocaleString();
}

/**
 * Map a task's last-run status to a terminal status glyph + label.
 * ✓ for success-ish, ✗ for failure-ish, ● for anything else / unknown.
 */
export function lastStatusGlyph(status?: string): { glyph: string; label: string } {
  const s = (status ?? '').toLowerCase();
  if (!s) return { glyph: '●', label: 'never run' };
  if (s.includes('fail') || s.includes('error')) return { glyph: '✗', label: status as string };
  if (s.includes('complet') || s === 'ok' || s.includes('success')) {
    return { glyph: '✓', label: status as string };
  }
  return { glyph: '●', label: status as string };
}
