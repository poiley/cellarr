import { describe, expect, it } from 'vitest';

import {
  activeDownloads,
  downloadProgress,
  healthSummary,
  historyEventV3Label,
  notableHealth,
  recentHistory,
  recentlyAdded,
  summarizeLibrary,
  upcomingItems,
  type CalendarItem,
} from '@app/_lib/dashboard';
import type {
  HealthCheck,
  HistoryRecordV3,
  Movie,
  QueueRecord,
  Series,
} from '@lib/api/types';

const movie = (over: Partial<Movie>): Movie =>
  ({
    id: 'm',
    title: 't',
    titleSlug: 't',
    year: 2020,
    tmdbId: 0,
    monitored: false,
    hasFile: false,
    status: 'released',
    path: '',
    rootFolderPath: '',
    sizeOnDisk: 0,
    qualityProfileId: null,
    added: '',
    tags: [],
    ...over,
  }) as Movie;

const series = (over: Partial<Series>): Series =>
  ({
    id: 's',
    title: 't',
    titleSlug: 't',
    tvdbId: 0,
    monitored: false,
    hasFile: false,
    status: 'continuing',
    seriesType: 'standard',
    path: '',
    rootFolderPath: '',
    sizeOnDisk: 0,
    qualityProfileId: null,
    added: '',
    tags: [],
    ...over,
  }) as Series;

describe('summarizeLibrary', () => {
  it('rolls up totals, monitored, missing, and size', () => {
    const movies = [
      movie({ monitored: true, hasFile: true, sizeOnDisk: 1000 }),
      movie({ monitored: true, hasFile: false }), // missing
      movie({ monitored: false, hasFile: false }),
    ];
    const tv = [series({ monitored: true, hasFile: false, sizeOnDisk: 500 })]; // missing

    const s = summarizeLibrary(movies, tv);
    expect(s.total).toBe(4);
    expect(s.monitored).toBe(3);
    expect(s.withFile).toBe(1);
    expect(s.missing).toBe(2);
    expect(s.sizeOnDisk).toBe(1500);
  });

  it('handles empty libraries', () => {
    expect(summarizeLibrary([], [])).toEqual({
      total: 0,
      monitored: 0,
      withFile: 0,
      missing: 0,
      sizeOnDisk: 0,
    });
  });
});

describe('activeDownloads', () => {
  it('keeps in-flight downloads and drops scheduled command tasks', () => {
    const records: QueueRecord[] = [
      { id: '1', title: 'A', status: 'downloading', protocol: 'torrent', size: 100, sizeleft: 40 },
      { id: '2', title: 'RssSync', status: 'scheduled', protocol: 'unknown' },
      { id: '3', title: 'B', status: 'completed', protocol: 'usenet', sizeleft: 0 },
      { id: '4', title: 'C', status: 'queued', protocol: 'torrent' },
    ];
    const active = activeDownloads(records);
    expect(active.map((r) => r.id)).toEqual(['1', '4']);
  });

  it('treats positive sizeleft as active even with an unknown status', () => {
    const records: QueueRecord[] = [
      { id: '1', title: 'A', status: 'weird', protocol: 'torrent', size: 10, sizeleft: 5 },
    ];
    expect(activeDownloads(records)).toHaveLength(1);
  });
});

describe('downloadProgress', () => {
  it('computes a [0,1] fraction from size/sizeleft', () => {
    expect(
      downloadProgress({ id: '1', title: '', status: '', protocol: '', size: 100, sizeleft: 25 })
    ).toBeCloseTo(0.75);
  });

  it('returns undefined when size is missing or zero', () => {
    expect(downloadProgress({ id: '1', title: '', status: '', protocol: '' })).toBeUndefined();
    expect(
      downloadProgress({ id: '1', title: '', status: '', protocol: '', size: 0, sizeleft: 0 })
    ).toBeUndefined();
  });

  it('clamps to [0,1]', () => {
    expect(
      downloadProgress({ id: '1', title: '', status: '', protocol: '', size: 100, sizeleft: -10 })
    ).toBe(1);
  });
});

describe('notableHealth', () => {
  it('surfaces warnings, errors, and untyped checks; drops oks/notices', () => {
    const checks: HealthCheck[] = [
      { type: 'warning', message: 'w' },
      { type: 'error', message: 'e' },
      { type: 'ok', message: 'fine' },
      { type: 'notice', message: 'fyi' },
      { message: 'bare' },
    ];
    const out = notableHealth(checks);
    expect(out.map((c) => c.message)).toEqual(['w', 'e', 'bare']);
  });
});

describe('recentHistory', () => {
  it('sorts newest first and caps the list', () => {
    const recs: HistoryRecordV3[] = [
      { id: 'a', date: '2026-01-01T00:00:00Z' },
      { id: 'b', date: '2026-03-01T00:00:00Z' },
      { id: 'c', date: '2026-02-01T00:00:00Z' },
    ];
    const out = recentHistory(recs, 2);
    expect(out.map((r) => r.id)).toEqual(['b', 'c']);
  });
});

describe('historyEventV3Label', () => {
  it('maps known event types to readable labels', () => {
    expect(historyEventV3Label('grabbed')).toBe('Grabbed');
    expect(historyEventV3Label('downloadFolderImported')).toBe('Imported');
    expect(historyEventV3Label('downloadFailed')).toBe('Download failed');
    expect(historyEventV3Label(undefined)).toBe('Event');
  });
});

describe('recentlyAdded', () => {
  it('keeps only monitored items, newest added first, capped', () => {
    const movies = [
      movie({ id: 'm1', title: 'Old', year: 1999, monitored: true, added: '2026-01-01T00:00:00Z' }),
      movie({ id: 'm2', title: 'Unmon', monitored: false, added: '2026-06-01T00:00:00Z' }),
    ];
    const tv = [
      series({ id: 's1', title: 'New', monitored: true, hasFile: true, added: '2026-06-10T00:00:00Z' }),
    ];
    const out = recentlyAdded(movies, tv, 5);
    expect(out.map((r) => r.id)).toEqual(['s1', 'm1']);
    expect(out[0]).toMatchObject({ kind: 'series', hasFile: true });
    expect(out[1].title).toBe('Old (1999)');
  });

  it('sorts items without an added timestamp last and respects the limit', () => {
    const movies = [
      movie({ id: 'a', monitored: true, added: '' }),
      movie({ id: 'b', monitored: true, added: '2026-05-01T00:00:00Z' }),
      movie({ id: 'c', monitored: true, added: '2026-06-01T00:00:00Z' }),
    ];
    const out = recentlyAdded(movies, [], 2);
    expect(out.map((r) => r.id)).toEqual(['c', 'b']);
  });
});

describe('healthSummary', () => {
  it('reports OK with a ● glyph when there are no notable checks', () => {
    const s = healthSummary([{ type: 'ok', message: 'fine' }]);
    expect(s).toEqual({ glyph: '●', word: 'OK', hasWarnings: false, count: 0 });
  });

  it('reports a ▲ glyph and a pluralized count when warnings exist', () => {
    const one = healthSummary([{ type: 'warning', message: 'w' }]);
    expect(one).toMatchObject({ glyph: '▲', word: '1 warning', hasWarnings: true, count: 1 });

    const many = healthSummary([
      { type: 'warning', message: 'w' },
      { type: 'error', message: 'e' },
    ]);
    expect(many).toMatchObject({ glyph: '▲', word: '2 warnings', count: 2 });
  });
});

describe('upcomingItems', () => {
  it('drops undated rows, sorts earliest first, and caps', () => {
    const items: CalendarItem[] = [
      { id: '1', title: 'B', airDate: '2026-06-20' },
      { id: '2', title: 'no-date' },
      { id: '3', title: 'A', date: '2026-06-10' },
      { id: '4', title: 'C', airDate: '2026-06-25' },
    ];
    const out = upcomingItems(items, 2);
    expect(out.map((i) => i.id)).toEqual(['3', '1']);
  });
});
