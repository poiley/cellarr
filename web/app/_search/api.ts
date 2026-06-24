'use client';

// Data glue for the Add / interactive-search screens. These talk to the
// Radarr/Sonarr-compatible `/api/v3` shim (crates/cellarr-api/src/shim.rs) via the
// shared CellarrClient — that is where lookup, add (POST movie/series), and the
// manual release search actually live. The native `/api/v1/lookup` route does NOT
// exist (it 404s to the SPA's index.html, which is the "Lookup failed" bug), so
// everything here is pinned to the working v3 endpoints discovered by curling the
// seeded daemon at :9494:
//
//   * GET  /api/v3/movie/lookup?term=…   → LookupCandidate[] (tmdbId)
//   * GET  /api/v3/series/lookup?term=…  → LookupCandidate[] (tvdbId)
//   * POST /api/v3/movie                 → create a monitored movie
//   * POST /api/v3/series                → create a monitored series
//   * GET  /api/v3/release?contentId=…   → ranked CandidateRelease[] (manual search)
//
// This module is data/routing glue, not a UI primitive, so it does not violate the
// SRCL-only rule (lint allowlist: relative imports + @lib/api/*).

import { api } from '@lib/api/client';
import type { LookupCandidate, MediaType } from '@lib/api/types';

/** A candidate title returned by a lookup (movie or series). */
export interface LookupResult {
  /**
   * A stable key for this candidate within the merged result set. Built from the
   * media type + the source foreign id (tmdb/tvdb) so movie 603 and a series with
   * tvdb 603 never collide.
   */
  foreign_id: string;
  title: string;
  year?: number;
  media_type?: MediaType;
  /** Short overview/description, when the metadata source provides one. */
  overview?: string;
  /** Whether this title is already monitored in a cellarr library. */
  already_added?: boolean;
  /** TMDB id (movies). Used to build the add POST. */
  tmdb_id?: number;
  /** TVDB id (series). Used to build the add POST. */
  tvdb_id?: number;
  /** URL-safe slug echoed back to the add POST. */
  title_slug?: string;
  /**
   * Source popularity (TMDB/TVDB-style). Used purely as a disambiguation /
   * ranking hint so the obvious hit floats to the top of its section. May be
   * absent — the metadata source does not always provide it.
   */
  popularity?: number;
  /** Average user rating (0–10), when the source provides one. */
  vote_average?: number;
  /** Runtime in minutes (movies), when known — another disambiguation hint. */
  runtime?: number;
}

/** The body the add call posts to create monitored content. */
export interface AddContentRequest {
  /** The media type decides which v3 endpoint we POST to (movie vs series). */
  media_type: MediaType;
  title: string;
  title_slug?: string;
  year?: number;
  tmdb_id?: number;
  tvdb_id?: number;
  /** Root folder to add the title under (e.g. "/movies"). */
  root_folder_path: string;
  quality_profile_id?: string;
  /** Whether the title is monitored on add. Defaults to true. */
  monitored?: boolean;
  /** Trigger an automatic search immediately after adding. */
  search_on_add?: boolean;
}

/** A candidate release surfaced by an interactive (manual) search. */
export interface CandidateRelease {
  guid: string;
  title: string;
  indexer?: string;
  protocol?: 'torrent' | 'usenet' | string;
  /** Parsed quality name (e.g. "Bluray-1080p"). */
  quality?: string;
  /** Total custom-format score for this release under the active profile. */
  cf_score?: number;
  /** Overall decision score / rank the candidate sorted on, when provided. */
  score?: number;
  /** Human-readable breakdown of how the score was reached. */
  score_reason?: string;
  size?: number;
  seeders?: number;
  /** Indexer flags (e.g. "freeleech"). */
  flags?: string[];
  /** True when cellarr would reject this release (with a reason). */
  rejected?: boolean;
  rejection_reason?: string;
}

/** Decide whether a lookup candidate is a series (vs a movie). */
function isSeriesCandidate(c: LookupCandidate): boolean {
  return c.tvdbId !== undefined && c.tvdbId !== null;
}

/** Read a numeric field off the loosely-typed lookup candidate, if present. */
function numField(c: LookupCandidate, ...keys: string[]): number | undefined {
  for (const k of keys) {
    const v = c[k];
    if (typeof v === 'number' && Number.isFinite(v)) return v;
  }
  return undefined;
}

/** Map a raw v3 lookup candidate to the UI's LookupResult shape. */
function toLookupResult(c: LookupCandidate, mediaType: MediaType): LookupResult {
  const sourceId =
    mediaType === 'tv' ? c.tvdbId ?? c.titleSlug : c.tmdbId ?? c.titleSlug;
  return {
    foreign_id: `${mediaType}:${sourceId ?? c.titleSlug}`,
    title: c.title,
    year: c.year,
    media_type: mediaType,
    overview: c.overview,
    already_added: c.monitored && c.hasFile ? true : c.monitored,
    tmdb_id: c.tmdbId,
    tvdb_id: c.tvdbId,
    title_slug: c.titleSlug,
    popularity: numField(c, 'popularity'),
    vote_average: numField(c, 'voteAverage', 'ratings', 'rating'),
    runtime: numField(c, 'runtime'),
  };
}

/**
 * Relevance/popularity ranking for a single section's results against the typed
 * query. Exact (case-insensitive) title matches win, then prefix matches, then
 * the more popular / higher-rated / more recent title — so the obvious hit lands
 * first and the long tail trails behind. Stable for ties (returns 0).
 */
export function rankResults(results: LookupResult[], term: string): LookupResult[] {
  const q = term.trim().toLowerCase();
  const score = (r: LookupResult): number => {
    const title = r.title.toLowerCase();
    let s = 0;
    if (q && title === q) s += 1_000_000;
    else if (q && title.startsWith(q)) s += 100_000;
    else if (q && title.includes(q)) s += 10_000;
    // Popularity dominates rating, which dominates recency, as a tiebreak.
    if (r.popularity !== undefined) s += Math.min(r.popularity, 9_999);
    if (r.vote_average !== undefined) s += r.vote_average * 10;
    if (r.year !== undefined) s += (r.year - 1900) / 1000;
    return s;
  };
  // Decorate-sort-undecorate keeps the sort stable across engines.
  return results
    .map((r, i) => ({ r, i, s: score(r) }))
    .sort((a, b) => b.s - a.s || a.i - b.i)
    .map((x) => x.r);
}

/**
 * Free-text lookup for titles to add. Fans out to BOTH the movie and series
 * lookups in parallel, tags each result with its media type, and merges them so
 * the Add screen surfaces a single ranked list. A failure of one surface does not
 * sink the other — partial results still render.
 */
export async function lookup(
  term: string,
  signal?: AbortSignal
): Promise<LookupResult[]> {
  const q = term.trim();
  if (!q) return [];

  const [movies, series] = await Promise.allSettled([
    api.requestV3<LookupCandidate[]>('/movie/lookup', { query: { term: q }, signal }),
    api.requestV3<LookupCandidate[]>('/series/lookup', { query: { term: q }, signal }),
  ]);

  // If BOTH lookups failed, surface the error so the screen can show its banner.
  if (movies.status === 'rejected' && series.status === 'rejected') {
    throw movies.reason;
  }

  const results: LookupResult[] = [];
  if (movies.status === 'fulfilled') {
    for (const c of movies.value ?? []) {
      // The movie lookup occasionally echoes series-shaped rows; honour the id.
      results.push(toLookupResult(c, isSeriesCandidate(c) ? 'tv' : 'movie'));
    }
  }
  if (series.status === 'fulfilled') {
    for (const c of series.value ?? []) {
      results.push(toLookupResult(c, 'tv'));
    }
  }
  return results;
}

/**
 * Create monitored content from a chosen lookup result by POSTing to the matching
 * v3 endpoint (`/movie` for movies, `/series` for everything else). Returns the
 * created node's id, which the interactive screen can search releases for.
 */
export async function addContent(
  body: AddContentRequest,
  signal?: AbortSignal
): Promise<{ id: string }> {
  const common = {
    title: body.title,
    titleSlug: body.title_slug,
    year: body.year,
    qualityProfileId: body.quality_profile_id,
    rootFolderPath: body.root_folder_path,
    monitored: body.monitored ?? true,
  };

  if (body.media_type === 'tv') {
    const created = await api.requestV3<{ id: string }>('/series', {
      method: 'POST',
      body: {
        ...common,
        tvdbId: body.tvdb_id,
        addOptions: { searchForMissingEpisodes: body.search_on_add ?? false },
      },
      signal,
    });
    return { id: created.id };
  }

  const created = await api.requestV3<{ id: string }>('/movie', {
    method: 'POST',
    body: {
      ...common,
      tmdbId: body.tmdb_id,
      addOptions: { searchForMovie: body.search_on_add ?? false },
    },
    signal,
  });
  return { id: created.id };
}

/**
 * Interactive/manual release search for a content node
 * (`GET /api/v3/release?contentId=…`). The shim returns ranked candidates with
 * parsed quality, custom-format score, a human score reason, and rejection state.
 */
export function searchReleases(
  contentId: string,
  signal?: AbortSignal
): Promise<CandidateRelease[]> {
  return api.requestV3<CandidateRelease[]>('/release', {
    query: { contentId },
    signal,
  });
}

/** Hand a chosen release to a download client (the manual grab). */
export function grabRelease(guid: string, contentId: string, signal?: AbortSignal) {
  return api.requestV3<{ accepted?: boolean }>('/release', {
    method: 'POST',
    body: { guid, contentId },
    signal,
  });
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
