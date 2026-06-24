// Screen-local helpers for the System / Status screen.
//
// The shared API client (web/lib/api/client.ts) does not yet model the v3
// scheduler-task surface (`GET /api/v3/system/task`), so this screen reaches it
// through the client's generic `requestV3` escape hatch. The shape below mirrors
// what the daemon serializes (crates/cellarr-api/src/shim.rs): a scheduled task
// with its interval, the countdown to the next run, and the (derived) last run.

import type { CellarrClient } from '@lib/api/client';

/** A scheduled task as returned by `GET /api/v3/system/task`. */
export interface SystemTask {
  id: number | string;
  name: string;
  /** The command name a 'Run now' POST targets (`POST /api/v3/command`). */
  taskName: string;
  /** Cadence in minutes. */
  interval: number;
  /** ISO timestamp of the next scheduled execution (the live countdown). */
  nextExecution: string;
  /** ISO timestamp of the previous execution; DERIVED by the daemon, may be null. */
  lastExecution: string | null;
  /** Duration string of the previous run (currently hardcoded '00:00:00'). */
  lastDuration?: string;
  /** Outcome of the previous run (real). */
  lastStatus?: string;
}

/** Fetch the scheduler task list off the v3 shim. */
export function fetchSystemTasks(
  client: CellarrClient,
  signal?: AbortSignal
): Promise<SystemTask[]> {
  return client.requestV3<SystemTask[]>('/system/task', { signal });
}

/**
 * Fire a 'Run now' for a task by POSTing its command to `/api/v3/command`.
 * Returns the accepted command resource (shape varies; we only need the call to
 * resolve for success feedback).
 */
export function runTaskNow(
  client: CellarrClient,
  taskName: string,
  signal?: AbortSignal
): Promise<unknown> {
  return client.requestV3<unknown>('/command', {
    method: 'POST',
    body: { name: taskName },
    signal,
  });
}

/** Format an ISO timestamp as a compact, locale-stable, monospace-friendly string. */
export function formatTimestamp(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  // YYYY-MM-DD HH:MM — stable, terminal-friendly, no locale punctuation drift.
  const pad = (n: number) => String(n).padStart(2, '0');
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}`
  );
}

/** A human "in N min / N h" countdown from now to the next execution. */
export function formatCountdown(iso: string | null | undefined): string {
  if (!iso) return '—';
  const target = new Date(iso).getTime();
  if (Number.isNaN(target)) return iso;
  const deltaMs = target - Date.now();
  if (deltaMs <= 0) return 'due';
  const mins = Math.round(deltaMs / 60000);
  if (mins < 60) return `in ${mins} min`;
  const hours = Math.floor(mins / 60);
  const rem = mins % 60;
  return rem ? `in ${hours} h ${rem} min` : `in ${hours} h`;
}

/** Format a cadence (minutes) compactly. */
export function formatInterval(minutes: number): string {
  if (!Number.isFinite(minutes) || minutes <= 0) return '—';
  if (minutes < 60) return `${minutes} min`;
  const hours = minutes / 60;
  if (Number.isInteger(hours)) return `${hours} h`;
  return `${minutes} min`;
}
