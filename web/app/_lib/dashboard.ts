// Dashboard aggregation helpers — pure functions that derive the at-a-glance
// summary numbers from the typed API payloads. Kept out of the page component so
// they can be unit-tested without React. No UI here; the page does the SRCL.

import { type CellarrClient } from '@lib/api/client';
import type {
  HealthCheck,
  HistoryRecordV3,
  Movie,
  QueueRecord,
  Series,
} from '@lib/api/types';

/** Monitored / missing rollup across movies + series libraries. */
export interface MonitoredSummary {
  total: number;
  monitored: number;
  withFile: number;
  /** Monitored items that have no file on disk yet (the "wanted" set). */
  missing: number;
  sizeOnDisk: number;
}

export function summarizeLibrary(
  movies: Movie[],
  series: Series[]
): MonitoredSummary {
  const items: Array<Movie | Series> = [...movies, ...series];
  const summary: MonitoredSummary = {
    total: items.length,
    monitored: 0,
    withFile: 0,
    missing: 0,
    sizeOnDisk: 0,
  };
  for (const it of items) {
    if (it.monitored) summary.monitored += 1;
    if (it.hasFile) summary.withFile += 1;
    if (it.monitored && !it.hasFile) summary.missing += 1;
    summary.sizeOnDisk += typeof it.sizeOnDisk === 'number' ? it.sizeOnDisk : 0;
  }
  return summary;
}

/**
 * Queue records that represent real, in-flight downloads (as opposed to the
 * scheduled command tasks the daemon also surfaces on the queue). A download is
 * "active" when it is downloading/queued/paused or reports bytes remaining.
 */
const ACTIVE_STATUSES = new Set([
  'downloading',
  'queued',
  'paused',
  'delay',
  'downloadclientunavailable',
  'warning',
]);

export function activeDownloads(records: QueueRecord[]): QueueRecord[] {
  return records.filter((r) => {
    const status = (r.status ?? '').toLowerCase();
    if (ACTIVE_STATUSES.has(status)) return true;
    return typeof r.sizeleft === 'number' && r.sizeleft > 0;
  });
}

/** Fraction [0,1] of a download that is complete, or undefined if unknown. */
export function downloadProgress(record: QueueRecord): number | undefined {
  // Prefer the client's explicit live percentage (0..100). A magnet still fetching
  // metadata reports 0 here — a real "0%", which we DO want to show (it's the
  // "stuck" signal), unlike the size/sizeleft path which can't distinguish 0% from
  // unknown when the advertised size is 0.
  if (typeof record.progress === 'number' && !Number.isNaN(record.progress)) {
    return Math.min(1, Math.max(0, record.progress / 100));
  }
  const size = typeof record.size === 'number' ? record.size : undefined;
  const left = typeof record.sizeleft === 'number' ? record.sizeleft : undefined;
  if (size === undefined || left === undefined || size <= 0) return undefined;
  const done = (size - left) / size;
  if (Number.isNaN(done)) return undefined;
  return Math.min(1, Math.max(0, done));
}

/** Health checks worth surfacing on the dashboard (warnings + errors). */
const NOTABLE_HEALTH = new Set(['warning', 'error']);

export function notableHealth(checks: HealthCheck[]): HealthCheck[] {
  return checks.filter((c) => {
    const type = (c.type ?? '').toLowerCase();
    return type === '' || NOTABLE_HEALTH.has(type);
  });
}

/** Most-recent history records, newest first, capped to `limit`. */
export function recentHistory(
  records: HistoryRecordV3[],
  limit = 6
): HistoryRecordV3[] {
  const sorted = [...records].sort((a, b) => {
    const da = a.date ? Date.parse(a.date) : 0;
    const db = b.date ? Date.parse(b.date) : 0;
    return db - da;
  });
  return sorted.slice(0, limit);
}

/** Human label for a v3 history eventType (grab / import / etc.). */
export function historyEventV3Label(eventType?: string): string {
  switch ((eventType ?? '').toLowerCase()) {
    case 'grabbed':
      return 'Grabbed';
    case 'downloadfolderimported':
    case 'imported':
      return 'Imported';
    case 'downloadfailed':
      return 'Download failed';
    case 'importfailed':
      return 'Import failed';
    case 'moviefiledeleted':
    case 'episodefiledeleted':
    case 'filedeleted':
      return 'Deleted';
    case 'moviefilerenamed':
    case 'episodefilerenamed':
    case 'filerenamed':
      return 'Renamed';
    case 'movieadded':
    case 'seriesadded':
      return 'Added';
    case '':
      return 'Event';
    default:
      return eventType as string;
  }
}

// ---------------------------------------------------------------------------
// Recently added
// ---------------------------------------------------------------------------

/** A flattened "recently added" row across movies + series. */
export interface RecentItem {
  id: string;
  title: string;
  kind: 'movie' | 'series';
  /** ISO timestamp the item was added to the library (may be empty). */
  added: string;
  monitored: boolean;
  hasFile: boolean;
}

/**
 * Whether an ISO timestamp is a usable, real "added" date.
 *
 * The v3 shim serializes a node with no recorded add time as the .NET/Radarr
 * sentinel `0001-01-01T00:00:00Z` (`MinValue`). Rendered naively that produces
 * the epoch artifact `12/31/1` in the UI. Treat the year-0001 sentinel (and any
 * absent/unparseable value) as "no date" so callers can omit it rather than
 * print garbage. A genuine timestamp is anything that parses and isn't the
 * sentinel.
 */
export function hasRealAddedDate(added: string | undefined): boolean {
  if (!added) return false;
  // The sentinel is exactly the year-0001 MinValue; match on the leading year so
  // any zone/format variant of it is caught too.
  if (added.startsWith('0001-01-01')) return false;
  const t = Date.parse(added);
  if (Number.isNaN(t)) return false;
  // Anything at/below the year-0001 boundary is the sentinel territory; a real
  // library add is always far later. Guard with a generous floor (year 1900).
  return t > Date.parse('1900-01-01T00:00:00Z');
}

/** Parse an `added` ISO timestamp to epoch ms, or 0 when absent/sentinel/invalid. */
function addedTime(added: string): number {
  if (!hasRealAddedDate(added)) return 0;
  const t = Date.parse(added);
  return Number.isNaN(t) ? 0 : t;
}

/**
 * Most-recently-added monitored items across both libraries, newest first.
 * Mirrors the originals' "Recently added" rail: we surface monitored adds (the
 * things the user actually cares to track), capped to `limit`. Items without an
 * `added` timestamp sort last.
 */
export function recentlyAdded(
  movies: Movie[],
  series: Series[],
  limit = 6
): RecentItem[] {
  // Normalize the `added` field to a real date or the empty string, so the page
  // can render the timestamp only when it is genuine (never the year-0001
  // sentinel that would otherwise print as the `12/31/1` epoch artifact).
  const realAdded = (added: string | undefined) =>
    hasRealAddedDate(added) ? (added as string) : '';
  const rows: RecentItem[] = [];
  for (const m of movies) {
    if (!m.monitored) continue;
    rows.push({
      id: m.id,
      title: m.year ? `${m.title} (${m.year})` : m.title,
      kind: 'movie',
      added: realAdded(m.added),
      monitored: m.monitored,
      hasFile: m.hasFile,
    });
  }
  for (const s of series) {
    if (!s.monitored) continue;
    rows.push({
      id: s.id,
      title: s.year ? `${s.title} (${s.year})` : s.title,
      kind: 'series',
      added: realAdded(s.added),
      monitored: s.monitored,
      hasFile: s.hasFile,
    });
  }
  rows.sort((a, b) => addedTime(b.added) - addedTime(a.added));
  return rows.slice(0, limit);
}

// ---------------------------------------------------------------------------
// Health glyph
// ---------------------------------------------------------------------------

/** A glyph + word health rollup — never colour-only (accessibility). */
export interface HealthStatus {
  /** ASCII status glyph: ● when OK, ▲ when there are warnings/errors. */
  glyph: '●' | '▲';
  /** Short label: 'OK' or 'N warnings'. */
  word: string;
  /** True when there is at least one notable check. */
  hasWarnings: boolean;
  count: number;
}

/** Summarize notable health checks into a glyph + word for the dashboard. */
export function healthSummary(checks: HealthCheck[]): HealthStatus {
  const notable = notableHealth(checks);
  const count = notable.length;
  if (count === 0) {
    return { glyph: '●', word: 'OK', hasWarnings: false, count: 0 };
  }
  return {
    glyph: '▲',
    word: `${count} ${count === 1 ? 'warning' : 'warnings'}`,
    hasWarnings: true,
    count,
  };
}

// ---------------------------------------------------------------------------
// Calendar (upcoming) — read via the generic v3 escape hatch
// ---------------------------------------------------------------------------

/**
 * A calendar row from `GET /api/v3/calendar`. The backend returns the originals'
 * shape (`id`, `title`, `airDate`/`airDateUtc`, `monitored`, `hasFile`) plus the
 * cellarr-native `date`/`summary` aliases. Today only TV daily-coded episodes
 * carry a self-contained date, so this is often empty — the dashboard then shows
 * "Recently added" instead. See the calendar handler in shim.rs.
 */
export interface CalendarItem {
  id?: string;
  title?: string;
  airDate?: string;
  airDateUtc?: string;
  monitored?: boolean;
  hasFile?: boolean;
  date?: string;
  summary?: string;
  [key: string]: unknown;
}

/** Format a `Date` as a `YYYY-MM-DD` window bound for the calendar query. */
function isoDate(d: Date): string {
  return d.toISOString().slice(0, 10);
}

/**
 * Fetch the upcoming calendar window [today, today+days]. Uses the client's
 * generic `requestV3` escape hatch because the calendar route is not yet a typed
 * method on the shared client (which screen agents must not edit).
 */
export async function fetchCalendar(
  client: CellarrClient,
  days = 14,
  signal?: AbortSignal
): Promise<CalendarItem[]> {
  const now = new Date();
  const end = new Date(now.getTime() + days * 24 * 60 * 60 * 1000);
  return client.requestV3<CalendarItem[]>('/calendar', {
    query: { start: isoDate(now), end: isoDate(end) },
    signal,
  });
}

/** Upcoming calendar rows sorted by date (earliest first), capped to `limit`. */
export function upcomingItems(items: CalendarItem[], limit = 6): CalendarItem[] {
  const dateOf = (i: CalendarItem) => i.airDate ?? i.date ?? '';
  return [...items]
    .filter((i) => dateOf(i))
    .sort((a, b) => dateOf(a).localeCompare(dateOf(b)))
    .slice(0, limit);
}
