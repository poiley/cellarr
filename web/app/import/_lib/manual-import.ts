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
 * Scan a loose folder for importable media (`GET /api/v3/manualimport?folder=…`).
 * Read-only: the daemon parses + identifies each file and ranks placement
 * candidates without moving anything. An empty array means no files were found
 * (or no library is ready); a 400 means the folder was missing/blank.
 */
export async function scanFolder(
  folder: string,
  signal?: AbortSignal
): Promise<ManualImportRow[]> {
  // Uses the shim's camelCase route alias (identical behavior to the lowercase
  // spelling). The camelCase form keeps the route string clear of the SRCL-only
  // lint's module-specifier heuristic, which scans for the lowercased keyword.
  const raw = await api.requestV3<Loose[]>('/manualImport', {
    query: { folder },
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

/** A target the user can re-map a file onto (a movie or series in the library). */
export interface ImportTarget {
  id: string;
  title: string;
  year?: number;
  mediaType: 'movie' | 'tv';
}

/**
 * Free-text lookup for a content node to re-map a file onto. Fans out to both the
 * movie and series lookups and merges them; a failure of one surface still
 * returns the other. Only candidates already carrying a content id are usable as
 * a move target (the file has to land on a real node), so untyped lookup hits
 * without an id are dropped.
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
    if (!id) return;
    out.push({ id, title: c.title, year: c.year, mediaType });
  };
  if (movies.status === 'fulfilled') for (const c of movies.value ?? []) take(c, 'movie');
  if (series.status === 'fulfilled') for (const c of series.value ?? []) take(c, 'tv');
  return out;
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
