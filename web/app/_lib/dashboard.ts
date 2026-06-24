// Dashboard aggregation helpers — pure functions that derive the at-a-glance
// summary numbers from the typed API payloads. Kept out of the page component so
// they can be unit-tested without React. No UI here; the page does the SRCL.

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
