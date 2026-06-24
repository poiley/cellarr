// Presentation helpers for the Library + Item-detail screens. Pure formatting
// glue over the loosely-typed `/api/v1` content shapes (docs/09-api.md) — no UI
// primitives here, so this stays SRCL-only-rule compliant.

import type { ContentNode, ContentRef, Library, MediaFile, Movie, Series } from '@lib/api/types';

function str(v: unknown): string | undefined {
  return typeof v === 'string' && v.length ? v : undefined;
}
function num(v: unknown): number | undefined {
  return typeof v === 'number' && Number.isFinite(v) ? v : undefined;
}

/**
 * Coerce any API value into a readable record. The `/api/v1` content shapes are
 * loosely typed (`[key: string]: unknown`), and the strongly-typed interfaces
 * (Library, ContentRef…) read fine through an index lookup, so callers pass the
 * value as-is and we read fields off it defensively here.
 */
function rec(v: unknown): Record<string, unknown> {
  return v && typeof v === 'object' ? (v as Record<string, unknown>) : {};
}

/** A node's structural role, e.g. `series` / `season` / `episode` / `movie`. */
export function kindOf(node: unknown): string | undefined {
  return str(rec(node).kind);
}

/** The media type carried on libraries and content (`movie` | `tv` | …). */
export function mediaTypeOf(item: unknown): string {
  return str(rec(item).media_type) ?? 'unknown';
}

/** A short human label for a content node, falling back through what's present. */
export function titleOf(item: unknown): string {
  const r = rec(item);
  const direct = str(r.title) ?? str(r.name);
  if (direct) return direct;
  const coords = coordsLabel(r.coords);
  if (coords) return coords;
  const id = str(r.id);
  return id ? `#${id.slice(0, 8)}` : '(untitled)';
}

/** Render the tagged `coords` union into a compact label (S02E15, Disc1/Trk3…). */
export function coordsLabel(coords: unknown): string | undefined {
  if (!coords || typeof coords !== 'object') return undefined;
  const c = rec(coords);
  const type = str(c.type);
  switch (type) {
    case 'movie':
      return undefined;
    case 'episode': {
      const s = num(c.season);
      const e = num(c.episode);
      if (s !== undefined && e !== undefined) {
        return `S${pad(s)}E${pad(e)}`;
      }
      return undefined;
    }
    case 'daily':
      return str(c.date);
    case 'seasonpack': {
      const s = num(c.season);
      return s !== undefined ? `Season ${s}` : undefined;
    }
    case 'absolute': {
      const n = num(c.number);
      return n !== undefined ? `#${n}` : undefined;
    }
    case 'track': {
      const d = num(c.disc);
      const t = num(c.track);
      if (t !== undefined) return d !== undefined ? `Disc ${d} · Track ${t}` : `Track ${t}`;
      return undefined;
    }
    case 'book': {
      const p = num(c.series_position);
      return p !== undefined ? `Book ${p}` : undefined;
    }
    default:
      return undefined;
  }
}

function pad(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

/** Monitored → a stable status token the screens badge. */
export function monitoredLabel(node: unknown): 'MONITORED' | 'UNMONITORED' {
  return rec(node).monitored === true ? 'MONITORED' : 'UNMONITORED';
}

/** Human-readable byte size. */
export function formatSize(bytes: unknown): string {
  const n = num(bytes);
  if (n === undefined) return '—';
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(v >= 10 ? 0 : 1)} ${units[i]}`;
}

/** A file's assessed quality name, when scored. */
export function qualityName(file: unknown): string {
  const q = rec(file).quality;
  if (q && typeof q === 'object') {
    const name = str(rec(q).name);
    if (name) return name;
  }
  return '—';
}

/** A file's custom-format score, when the engine has scored it. */
export function scoreLabel(file: unknown): string | undefined {
  const s = num(rec(file).custom_format_score);
  return s === undefined ? undefined : `${s > 0 ? '+' : ''}${s}`;
}

/** Just the trailing filename from an absolute on-disk path. */
export function basename(path: unknown): string {
  const p = str(path);
  if (!p) return '—';
  const parts = p.split(/[\\/]/);
  return parts[parts.length - 1] || p;
}

// ---------------------------------------------------------------------------
// Library content view-model.
//
// The Library browse screen shows the *actual* items in a library — the movies
// and series the user is tracking — not the sparse `/api/v1` content refs (which
// carry no title/year/quality and don't enumerate every title). The rich data
// lives on the v3 catalogues (`GET /api/v3/movie`, `GET /api/v3/series`), so we
// normalize a Movie/Series into a single row shape the screen can render and
// drill into. Drill-in works because the v3 ids resolve through `/api/v1/content/{id}`.
// ---------------------------------------------------------------------------

/** A normalized, render-ready row for the Library content view. */
export interface LibraryItem {
  id: string;
  title: string;
  year?: number;
  /** `movie` | `series` — what kind of catalogue entry this row came from. */
  kind: 'movie' | 'series';
  monitored: boolean;
  hasFile: boolean;
  /** Best-effort quality label (from the movie's imported file, when present). */
  quality?: string;
  /** Bytes on disk, when reported. */
  sizeOnDisk?: number;
  /** Root folder the entry lives under — used to scope it to a library. */
  rootFolderPath?: string;
  /** Poster/art URL, when the API surfaces one (fixtures currently don't). */
  poster?: string;
}

/** Pull a poster/cover URL out of whatever image shape the API might carry. */
export function posterOf(item: unknown): string | undefined {
  const r = rec(item);
  // Direct fields some Radarr-shaped payloads use.
  const direct = str(r.poster) ?? str(r.remotePoster);
  if (direct) return direct;
  // Radarr `images: [{ coverType, url, remoteUrl }]`.
  const images = r.images;
  if (Array.isArray(images)) {
    const cover =
      images.find((i) => str(rec(i).coverType) === 'poster') ?? images[0];
    return str(rec(cover).remoteUrl) ?? str(rec(cover).url);
  }
  return undefined;
}

/** Best-effort quality name off a v3 movie (its imported `movieFile.quality`). */
export function movieQuality(movie: unknown): string | undefined {
  const file = rec(movie).movieFile;
  const name = str(rec(rec(file).quality).name) ?? str(rec(rec(rec(file).quality).quality).name);
  return name;
}

/** Normalize a v3 Movie into a {@link LibraryItem}. */
export function movieToItem(movie: Movie): LibraryItem {
  const r = rec(movie);
  return {
    id: movie.id,
    title: titleOf(movie),
    year: num(r.year),
    kind: 'movie',
    monitored: r.monitored === true,
    hasFile: r.hasFile === true,
    quality: movieQuality(movie),
    sizeOnDisk: num(r.sizeOnDisk),
    rootFolderPath: str(r.rootFolderPath),
    poster: posterOf(movie),
  };
}

/** Normalize a v3 Series into a {@link LibraryItem}. */
export function seriesToItem(series: Series): LibraryItem {
  const r = rec(series);
  return {
    id: series.id,
    title: titleOf(series),
    year: num(r.year),
    kind: 'series',
    monitored: r.monitored === true,
    hasFile: r.hasFile === true,
    quality: undefined,
    sizeOnDisk: num(r.sizeOnDisk),
    rootFolderPath: str(r.rootFolderPath),
    poster: posterOf(series),
  };
}

/**
 * Does a catalogue entry belong to this library? Movies/series are tied to a
 * library by the root folder they live under; we treat a missing root folder on
 * either side as a match so a single-library install still shows everything.
 */
export function itemInLibrary(item: LibraryItem, lib: Library): boolean {
  const roots = Array.isArray(lib.root_folders) ? lib.root_folders : [];
  if (roots.length === 0 || !item.rootFolderPath) return true;
  return roots.some((root) => {
    const a = normRoot(root);
    const b = normRoot(item.rootFolderPath!);
    return b === a || b.startsWith(`${a}/`);
  });
}

function normRoot(p: string): string {
  return p.replace(/[\\/]+$/, '');
}

/** Monitored/file/quality state tokens for the row's status badges. */
export function fileLabel(item: LibraryItem): 'DOWNLOADED' | 'MISSING' {
  return item.hasFile ? 'DOWNLOADED' : 'MISSING';
}

export type { ContentNode, ContentRef, Library, MediaFile, Movie, Series };
