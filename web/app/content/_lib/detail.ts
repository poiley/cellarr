// Content-screen helpers: fetch the rich v3 detail resource for a node and flip
// its `monitored` flag. These wrap the typed client's `requestV3` escape hatch
// (docs/09-api.md) for the two routes the bare client does not yet model:
//
//   * GET /api/v3/movie/{id} | /series/{id}  -> full detail resource
//   * PUT /api/v3/movie/{id} | /series/{id}  body {monitored} -> refreshed detail
//
// The content node at /api/v1/content/{id} carries the structural shape but not
// the catalogue identity (title/overview/size/profile). The v3 detail resource —
// keyed by the SAME id the Library screen drills in with — does.

import { api, resolveBaseUrl } from '@lib/api/client';
import type { Movie, Series } from '@lib/api/types';

/** Which v3 catalogue a node belongs to, inferred from its media type / kind. */
export type DetailKind = 'movie' | 'series';

type Loose = Record<string, unknown>;

/**
 * Decide whether a content node resolves through the movie or series catalogue.
 * TV nodes (series/season/episode) refresh + toggle through the series resource;
 * everything else through the movie resource.
 */
export function detailKindFor(node: Loose | undefined): DetailKind {
  if (!node) return 'movie';
  const kind = node.kind;
  if (
    node.media_type === 'tv' ||
    kind === 'series' ||
    kind === 'season' ||
    kind === 'episode'
  ) {
    return 'series';
  }
  return 'movie';
}

/** The detail resource the screen renders — a v3 Movie or Series. */
export type Detail = Movie | Series;

/** Fetch the rich v3 detail resource for a node id. */
export function getDetail(
  kind: DetailKind,
  id: string,
  signal?: AbortSignal
): Promise<Detail> {
  const path = kind === 'series' ? `/series/${id}` : `/movie/${id}`;
  return api.requestV3<Detail>(path, { signal });
}

/** PUT the monitored flag; resolves to the refreshed detail resource. */
export function setMonitored(
  kind: DetailKind,
  id: string,
  monitored: boolean,
  signal?: AbortSignal
): Promise<Detail> {
  const path = kind === 'series' ? `/series/${id}` : `/movie/${id}`;
  return api.requestV3<Detail>(path, { method: 'PUT', body: { monitored }, signal });
}

/**
 * Rewrite a content node's tag set via `PUT /api/v3/movie/{id}` | `/series/{id}`.
 * `tags` present replaces the WHOLE set (an empty array clears every tag); the
 * monitored flag is left untouched (omitted from the body). Resolves to the
 * refreshed detail resource, whose `tags` reflects the new set.
 */
export function setContentTags(
  kind: DetailKind,
  id: string,
  tags: number[],
  signal?: AbortSignal
): Promise<Detail> {
  const path = kind === 'series' ? `/series/${id}` : `/movie/${id}`;
  return api.requestV3<Detail>(path, { method: 'PUT', body: { tags }, signal });
}

/**
 * The Sonarr `seriesType` (`standard`/`daily`/`anime`) — the series numbering
 * model. `anime` turns on absolute-numbering + scene-remap and the anime
 * episode-file naming format.
 */
export type SeriesTypeValue = 'standard' | 'daily' | 'anime';

/**
 * Rewrite a series' `seriesType` via `PUT /api/v3/series/{id}`. The shim's
 * content update reads `seriesType` as a partial field (omitting it leaves the
 * value untouched), so this PUT carries ONLY `seriesType` — it never disturbs
 * the monitored flag or tags. Resolves to the refreshed detail resource, whose
 * `seriesType` reflects the new value. Only meaningful for a series node.
 */
export function setSeriesType(
  id: string,
  seriesType: SeriesTypeValue,
  signal?: AbortSignal
): Promise<Detail> {
  return api.requestV3<Detail>(`/series/${id}`, {
    method: 'PUT',
    body: { seriesType },
    signal,
  });
}

/** The result of toggling a season's monitored flag. */
export interface SeasonMonitorResult {
  seasonId: string;
  monitored: boolean;
  /** How many episode children the toggle cascaded to. */
  episodesUpdated: number;
}

/**
 * Toggle monitoring for a single season and (the Sonarr behavior) every episode
 * beneath it via `PUT /api/v3/season/monitor`. The id is the season content id —
 * the same id the structure tree drills with. Returns the cascade count.
 */
export function setSeasonMonitored(
  seasonId: string,
  monitored: boolean,
  signal?: AbortSignal
): Promise<SeasonMonitorResult> {
  return api.requestV3<SeasonMonitorResult>('/season/monitor', {
    method: 'PUT',
    body: { seasonId, monitored },
    signal,
  });
}

/** The result of toggling one or more episodes' monitored flag. */
export interface EpisodeMonitorResult {
  updated: number;
  monitored: boolean;
}

/**
 * Toggle monitoring for a set of episodes via `PUT /api/v3/episode/monitor`.
 * Unknown ids are skipped server-side (idempotent), so re-issuing on a removed
 * episode still succeeds. Returns how many episodes were persisted.
 */
export function setEpisodesMonitored(
  episodeIds: string[],
  monitored: boolean,
  signal?: AbortSignal
): Promise<EpisodeMonitorResult> {
  return api.requestV3<EpisodeMonitorResult>('/episode/monitor', {
    method: 'PUT',
    body: { episodeIds, monitored },
    signal,
  });
}

function str(v: unknown): string | undefined {
  return typeof v === 'string' && v.length ? v : undefined;
}
function num(v: unknown): number | undefined {
  return typeof v === 'number' && Number.isFinite(v) ? v : undefined;
}

/** A flattened, render-ready view of the metadata block above Files. */
export interface DetailView {
  title?: string;
  year?: number;
  /** Runtime in minutes (v3 exposes it this way); 0/absent means unknown. */
  runtime?: number;
  overview?: string;
  qualityProfileId?: string;
  sizeOnDisk?: number;
  status?: string;
  hasFile?: boolean;
  monitored: boolean;
  path?: string;
  /** The TMDB id for a movie node (links out to themoviedb.org); absent for series. */
  tmdbId?: number;
  /** The content node's assigned tag ids (`content_tag` join table). */
  tags: number[];
  /** Genres resolved by the metadata source (empty when unresolved). */
  genres: string[];
  /** Primary-source user rating on a 0–10 scale (TMDB vote_average), when known. */
  rating?: number;
  /** Number of votes backing `rating`, when known. */
  ratingVotes?: number;
  /**
   * The Sonarr `seriesType` (`standard`/`daily`/`anime`) for a series node; the
   * v3 series resource carries it, a movie resource does not (so it is absent).
   */
  seriesType?: string;
}

/** Project a v3 detail resource into the metadata-block view-model. */
export function toDetailView(detail: Detail | undefined): DetailView | undefined {
  if (!detail) return undefined;
  const r = detail as Loose;
  return {
    title: str(r.title),
    year: num(r.year),
    runtime: num(r.runtime),
    overview: str(r.overview),
    qualityProfileId: str(r.qualityProfileId),
    sizeOnDisk: num(r.sizeOnDisk),
    status: str(r.status),
    hasFile: r.hasFile === true,
    monitored: r.monitored === true,
    path: str(r.path),
    tmdbId: num(r.tmdbId),
    tags: Array.isArray(r.tags)
      ? r.tags.filter((t): t is number => typeof t === 'number')
      : [],
    genres: Array.isArray(r.genres)
      ? r.genres.filter((g): g is string => typeof g === 'string')
      : [],
    rating: num(tmdbRating(r)?.value),
    ratingVotes: num(tmdbRating(r)?.votes),
    seriesType: str(r.seriesType),
  };
}

/** Pull the TMDB rating object out of the v3 `ratings` map, if present. */
function tmdbRating(r: Loose): { value?: unknown; votes?: unknown } | undefined {
  const ratings = r.ratings;
  if (ratings && typeof ratings === 'object' && 'tmdb' in ratings) {
    const tmdb = (ratings as { tmdb?: unknown }).tmdb;
    if (tmdb && typeof tmdb === 'object') {
      return tmdb as { value?: unknown; votes?: unknown };
    }
  }
  return undefined;
}

/**
 * The cached-artwork URL for a node's poster/fanart, served by
 * `GET /api/v3/mediacover/{contentId}/{kind}` (crates/cellarr-api/src/mediacover.rs).
 * Returns a same-origin-or-base-prefixed URL the screen can drop straight into an
 * `<img src>`; the endpoint 404s when no artwork is cached, which the screen
 * handles by swapping in an ASCII placeholder on the image's error event.
 */
export function mediaCoverUrl(kind: 'poster' | 'fanart', id: string): string {
  const base = resolveBaseUrl();
  return `${base}/api/v3/mediacover/${encodeURIComponent(id)}/${kind}`;
}
