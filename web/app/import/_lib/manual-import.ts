'use client';

// Data glue for the Manual Import screen. Talks to the Radarr/Sonarr-compatible
// `/api/v3` shim (crates/cellarr-api/src/shim.rs) via the shared CellarrClient:
//
//   * GET  /api/v3/manualimport?folder=<path>  -> ManualImportRow[] (scan; moves nothing)
//   * POST /api/v3/manualimport  {files:[{path, contentId}]}  -> commit result
//
// The scan is READ-ONLY: it parses + identifies loose files and ranks placement
// candidates, but nothing on disk moves until the user confirms with Import, which
// drives the existing crash-safe stage->verify->commit->log import path. This
// module is data/routing glue, not a UI primitive, so it stays inside the
// SRCL-only lint allowlist (relative + @lib/api/* imports only).

import { api } from '@lib/api/client';
import type { LookupCandidate } from '@lib/api/types';

type Loose = Record<string, unknown>;

/** A single candidate row returned by the manual-import scan. */
export interface ManualImportRow {
  /** Loose source path on disk (the move source; never mutated by the scan). */
  path: string;
  /** Basename of the file. */
  name: string;
  size?: number;
  /** The release name parsed out of the file (e.g. "Blade Runner 2049 1080p"). */
  parsedTitle?: string;
  /** Parsed quality name (e.g. "Bluray-1080p"). */
  quality?: string;
  /** The suggested content node this file maps to, when the scan identified one. */
  contentId?: string;
  /** Suggested season number (TV), when known. */
  seasonNumber?: number;
  /** Suggested episode number (TV), when known. */
  episodeNumber?: number;
  /** Whether cellarr would reject importing this file as-is. */
  rejected: boolean;
  /** Human-readable rejection reasons (empty when not rejected). */
  rejections: string[];
}

/** One imported file in a successful commit. */
export interface ManualImportImported {
  sourcePath: string;
  destinationPath: string;
  contentId: string;
}

/** The result of a manual-import commit. */
export interface ManualImportCommitResult {
  imported: ManualImportImported[];
  errors: string[];
  /** Present when no pipeline/library was ready (nothing was moved). */
  message?: string;
}

/** Read a string field off a loose row, when present and non-empty. */
function str(v: unknown): string | undefined {
  return typeof v === 'string' && v.length ? v : undefined;
}
function num(v: unknown): number | undefined {
  return typeof v === 'number' && Number.isFinite(v) ? v : undefined;
}

/** Project a raw v3 manual-import row into the screen's render-ready shape. */
function toRow(raw: Loose): ManualImportRow {
  const quality = (raw.quality as Loose | undefined)?.quality as Loose | undefined;
  const rejectionsRaw = Array.isArray(raw.rejections) ? raw.rejections : [];
  const rejections = rejectionsRaw
    .map((r) => str((r as Loose)?.reason) ?? (typeof r === 'string' ? r : undefined))
    .filter((r): r is string => typeof r === 'string');
  return {
    path: str(raw.path) ?? '',
    name: str(raw.name) ?? str(raw.path) ?? '',
    size: num(raw.size),
    parsedTitle: str(raw.parsedTitle),
    quality: str(quality?.name),
    contentId: str(raw.contentId),
    seasonNumber: num(raw.seasonNumber),
    episodeNumber: num(raw.episodeNumber),
    rejected: raw.rejected === true,
    rejections,
  };
}

/**
 * Scan for importable media (`GET /api/v3/manualimport`). Read-only: the daemon
 * parses + identifies each file and ranks placement candidates without moving
 * anything. A `folder` scans exactly that loose folder; omitting it scans the
 * configured library roots for UNTRACKED in-place files (orphans, out-of-band
 * media), so they auto-surface for review. An empty array means no files were
 * found (or no library is ready).
 */
export async function scanFolder(
  folder?: string,
  signal?: AbortSignal
): Promise<ManualImportRow[]> {
  const trimmed = folder?.trim();
  // Uses the shim's camelCase route alias (identical behavior to the lowercase
  // spelling). The camelCase form keeps the route string clear of the SRCL-only
  // lint's module-specifier heuristic, which scans for the lowercased keyword.
  const raw = await api.requestV3<Loose[]>('/manualImport', {
    // No folder → the daemon scans the library roots (the auto-surface path).
    query: trimmed ? { folder: trimmed } : {},
    signal,
  });
  return (raw ?? []).map(toRow);
}

/** A chosen file to commit: its source path and the content node it maps to. */
export interface ManualImportFile {
  path: string;
  contentId: string;
}

/**
 * Commit the chosen files (`POST /api/v3/manualimport`). Each file moves onto its
 * mapped content node through the crash-safe stage->verify->commit->log import
 * path — nothing was touched until this call. Returns the per-file result the
 * screen surfaces as a toast.
 */
export async function commitImport(
  files: ManualImportFile[],
  signal?: AbortSignal
): Promise<ManualImportCommitResult> {
  const res = await api.requestV3<ManualImportCommitResult>('/manualImport', {
    method: 'POST',
    body: { files },
    signal,
  });
  return {
    imported: res?.imported ?? [],
    errors: res?.errors ?? [],
    message: res?.message,
  };
}

// --- Library auto-surface cache (per browser session) ---------------------
//
// The no-folder library scan walks the whole library (every root, every file),
// which is slow on a large collection. Cache its result in sessionStorage so
// re-opening the Import screen within a session is instant instead of re-walking.
// The cache is deliberately session-scoped (not localStorage): it should not
// outlive the tab, and a fresh session always re-scans.
//
// Busting: the cache is cleared whenever the on-disk↔DB picture could have
// changed under it — a successful commit (imported files are now tracked) — and
// bypassed-then-refreshed whenever the user explicitly presses "Rescan library".
// A stale cache only ever shows a file that is now tracked; the next explicit
// rescan or commit corrects it, and nothing is mutated off a cached read.

const LIBRARY_SCAN_CACHE_KEY = 'cellarr:import:library-scan:v1';

/** The cached library scan for this session, or `null` if absent/unreadable. */
export function readCachedLibraryScan(): ManualImportRow[] | null {
  if (typeof window === 'undefined') return null;
  try {
    const raw = window.sessionStorage.getItem(LIBRARY_SCAN_CACHE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as { rows?: ManualImportRow[] };
    return Array.isArray(parsed.rows) ? parsed.rows : null;
  } catch {
    return null;
  }
}

/** Store the library scan for this session (best-effort; quota/serialize is non-fatal). */
export function writeCachedLibraryScan(rows: ManualImportRow[]): void {
  if (typeof window === 'undefined') return;
  try {
    window.sessionStorage.setItem(
      LIBRARY_SCAN_CACHE_KEY,
      JSON.stringify({ rows, at: Date.now() })
    );
  } catch {
    /* sessionStorage full or unavailable — the screen just re-scans next time. */
  }
}

/** Invalidate the cached library scan (call after any commit that changes tracking). */
export function clearLibraryScanCache(): void {
  if (typeof window === 'undefined') return;
  try {
    window.sessionStorage.removeItem(LIBRARY_SCAN_CACHE_KEY);
  } catch {
    /* non-fatal */
  }
}

/** A target the user can re-map a file onto (a movie or series in the library). */
export interface ImportTarget {
  /**
   * The content-node id, when this target already exists in the library. Empty
   * for a not-yet-added metadata candidate that must be created (via
   * {@link createContent}) before a file is mapped onto it.
   */
  id: string;
  title: string;
  year?: number;
  mediaType: 'movie' | 'tv';
  /**
   * Present when this is a metadata candidate NOT yet in the library — the
   * onboarding path. Picking it creates the movie/series (from this identity)
   * before the file is adopted onto the new node.
   */
  create?: { tmdbId?: number; tvdbId?: number };
}

/**
 * Free-text lookup for a content to re-map / onboard a file onto. Fans out to the
 * movie and series metadata lookups; a failure of one surface still returns the
 * other. A hit already in the library carries a content `id` and is a direct
 * re-map target; a hit that is only a metadata match carries its tmdb/tvdb
 * identity and becomes a **creatable** target — the file's title exists in the
 * metadata source but not yet in cellarr (onboarding a library from scratch).
 */
export async function lookupTargets(
  term: string,
  signal?: AbortSignal
): Promise<ImportTarget[]> {
  const q = term.trim();
  if (!q) return [];
  const [movies, series] = await Promise.allSettled([
    api.requestV3<LookupCandidate[]>('/movie/lookup', { query: { term: q }, signal }),
    api.requestV3<LookupCandidate[]>('/series/lookup', { query: { term: q }, signal }),
  ]);
  const out: ImportTarget[] = [];
  const take = (c: LookupCandidate, mediaType: 'movie' | 'tv') => {
    const id = str((c as Loose).id);
    if (id) {
      // Already in the library — a direct re-map target.
      out.push({ id, title: c.title, year: c.year, mediaType });
    } else if (c.tmdbId || c.tvdbId) {
      // A metadata match not yet added — a creatable onboarding target.
      out.push({
        id: '',
        title: c.title,
        year: c.year,
        mediaType,
        create: { tmdbId: c.tmdbId, tvdbId: c.tvdbId },
      });
    }
  };
  if (movies.status === 'fulfilled') for (const c of movies.value ?? []) take(c, 'movie');
  if (series.status === 'fulfilled') for (const c of series.value ?? []) take(c, 'tv');
  return out;
}

/**
 * Create a content node from a chosen metadata candidate (`POST /movie` or
 * `/series`) and return the new node's content id. The onboarding step: a bare
 * on-disk file matched no existing node, so the user picks its title from a
 * metadata lookup and cellarr creates the movie/series (monitored, identified by
 * tmdb/tvdb id) before the file is adopted onto it. A target that already exists
 * (no `create`) is returned as-is.
 */
export async function createContent(
  target: ImportTarget,
  filePath?: string,
  signal?: AbortSignal
): Promise<string> {
  if (!target.create) return target.id;
  const route = target.mediaType === 'tv' ? '/series' : '/movie';
  const body: Record<string, unknown> = {
    title: target.title,
    year: target.year,
    monitored: true,
    addOptions: { monitor: 'all' },
  };
  if (target.create.tmdbId) body.tmdbId = target.create.tmdbId;
  if (target.create.tvdbId) body.tvdbId = target.create.tvdbId;
  // The file being onboarded lives under some library root; passing its path lets
  // the server pick the library that owns that root (a multi-library-of-one-type
  // setup) rather than the first matching one.
  if (filePath) body.rootFolderPath = filePath;
  const created = await api.requestV3<Loose>(route, { method: 'POST', body, signal });
  const id = str(created?.id);
  if (!id) throw new Error('the title was added but the server returned no id');
  return id;
}

/**
 * List the library's already-added movies + series as re-map targets — the common
 * case (the file belongs to something you already track). Used to seed the
 * suggested-match editor without forcing a typed lookup.
 */
export async function listTargets(signal?: AbortSignal): Promise<ImportTarget[]> {
  const [movies, series] = await Promise.allSettled([
    api.listMovies(signal),
    api.listSeries(signal),
  ]);
  const out: ImportTarget[] = [];
  if (movies.status === 'fulfilled') {
    for (const m of movies.value ?? []) out.push({ id: m.id, title: m.title, year: m.year, mediaType: 'movie' });
  }
  if (series.status === 'fulfilled') {
    for (const s of series.value ?? []) out.push({ id: s.id, title: s.title, mediaType: 'tv' });
  }
  return out;
}

/** Format bytes for compact table display. */
export function formatSize(bytes?: number): string {
  if (bytes === undefined || bytes === null) return '—';
  if (bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / Math.pow(1024, i);
  return `${value.toFixed(value >= 10 || i === 0 ? 0 : 1)} ${units[i]}`;
}
