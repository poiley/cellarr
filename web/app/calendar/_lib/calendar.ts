// Calendar screen helpers — pure functions that group the daemon's dated
// calendar rows (`GET /api/v3/calendar`) into per-day buckets the screen renders.
// No UI here so this is unit-testable and SRCL-only-rule compliant.
//
// The calendar endpoint returns one row per content node whose coordinates carry
// an air/release date (a movie release date, or a TV episode air date), within
// the queried window, sorted by date. See crates/cellarr-api/src/calendar.rs and
// the JSON handler in shim.rs. Each row mirrors the originals' shape (`id`,
// `title`, `airDate`/`airDateUtc`, `monitored`, `hasFile`) plus the cellarr-native
// `date`/`summary` aliases.

import type { CalendarItem } from '@app/_lib/dashboard';

export type { CalendarItem };

/** A normalized, render-ready calendar entry for one dated item. */
export interface CalendarEntry {
  id: string;
  /** The display label — episode-coded summary or movie title. */
  title: string;
  /** ISO `yyyy-mm-dd` day the item is dated to. */
  date: string;
  monitored: boolean;
  hasFile: boolean;
}

/** All items dated to one calendar day, in API (date-sorted) order. */
export interface CalendarDay {
  /** ISO `yyyy-mm-dd` key for the day. */
  date: string;
  entries: CalendarEntry[];
}

/** Pull the `yyyy-mm-dd` day out of a calendar row (airDate/date, or airDateUtc). */
export function dayOf(item: CalendarItem): string | undefined {
  const direct = item.airDate ?? item.date;
  if (typeof direct === 'string' && direct.length >= 10) return direct.slice(0, 10);
  const utc = item.airDateUtc;
  if (typeof utc === 'string' && utc.length >= 10) return utc.slice(0, 10);
  return undefined;
}

/** Normalize a raw calendar row into a {@link CalendarEntry}, or undefined when undated. */
export function toEntry(item: CalendarItem, index: number): CalendarEntry | undefined {
  const date = dayOf(item);
  if (!date) return undefined;
  const title =
    (typeof item.title === 'string' && item.title) ||
    (typeof item.summary === 'string' && item.summary) ||
    'Untitled';
  return {
    id: typeof item.id === 'string' && item.id ? item.id : `${date}-${index}`,
    title,
    date,
    monitored: item.monitored === true,
    hasFile: item.hasFile === true,
  };
}

/**
 * Group dated calendar rows into per-day buckets, sorted by day ascending
 * (earliest first). Undated rows are dropped (an undated item is not a calendar
 * entry). Within a day, entries keep their incoming (already date-sorted) order.
 */
export function groupByDay(items: CalendarItem[]): CalendarDay[] {
  const byDay = new Map<string, CalendarEntry[]>();
  items.forEach((item, i) => {
    const entry = toEntry(item, i);
    if (!entry) return;
    const bucket = byDay.get(entry.date) ?? [];
    bucket.push(entry);
    byDay.set(entry.date, bucket);
  });
  return [...byDay.entries()]
    .sort((a, b) => a[0].localeCompare(b[0]))
    .map(([date, entries]) => ({ date, entries }));
}

/** The total number of dated entries across all day buckets. */
export function countEntries(days: CalendarDay[]): number {
  return days.reduce((n, d) => n + d.entries.length, 0);
}

/**
 * A human heading for a day key: `Weekday, Mon D` plus a `Today` / `Tomorrow`
 * relative hint when applicable. `now` is injectable for deterministic tests.
 */
export function dayHeading(date: string, now: Date = new Date()): string {
  // Parse as a local date (the row is a plain `yyyy-mm-dd`, not an instant).
  const [y, m, d] = date.split('-').map((p) => Number.parseInt(p, 10));
  if (!y || !m || !d) return date;
  const day = new Date(y, m - 1, d);
  if (Number.isNaN(day.getTime())) return date;

  const startOf = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate());
  const diffDays = Math.round(
    (startOf(day).getTime() - startOf(now).getTime()) / (24 * 60 * 60 * 1000)
  );
  const label = day.toLocaleDateString(undefined, {
    weekday: 'short',
    month: 'short',
    day: 'numeric',
  });
  if (diffDays === 0) return `${label} · Today`;
  if (diffDays === 1) return `${label} · Tomorrow`;
  return label;
}

/** Format a `Date` as a `yyyy-mm-dd` window bound for the calendar query. */
export function isoDate(d: Date): string {
  return d.toISOString().slice(0, 10);
}
