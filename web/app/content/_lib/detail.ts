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

import { api } from '@lib/api/client';
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
  overview?: string;
  qualityProfileId?: string;
  sizeOnDisk?: number;
  status?: string;
  hasFile?: boolean;
  monitored: boolean;
  path?: string;
}

/** Project a v3 detail resource into the metadata-block view-model. */
export function toDetailView(detail: Detail | undefined): DetailView | undefined {
  if (!detail) return undefined;
  const r = detail as Loose;
  return {
    title: str(r.title),
    year: num(r.year),
    overview: str(r.overview),
    qualityProfileId: str(r.qualityProfileId),
    sizeOnDisk: num(r.sizeOnDisk),
    status: str(r.status),
    hasFile: r.hasFile === true,
    monitored: r.monitored === true,
    path: str(r.path),
  };
}
