'use client';

// Data glue for the Add / interactive-search screens. These talk to the native
// `/api/v1` lookup + command endpoints through the shared CellarrClient's generic
// `request` helper (the typed helpers don't yet cover lookup/release search, so we
// model the shapes the UI reads locally — see docs/09-api.md and the Rust
// `Release` / `SearchTerms` shapes in crates/cellarr-core).
//
// This module is data/routing glue, not a UI primitive, so it does not violate the
// SRCL-only rule (lint allowlist: relative imports + @lib/api/*).

import { api } from '@lib/api/client';
import type { MediaType } from '@lib/api/types';

/** A candidate title returned by a lookup (movie/series/album/book). */
export interface LookupResult {
  /** Foreign id the add call references (e.g. tmdb/tvdb/musicbrainz id). */
  foreign_id: string;
  title: string;
  year?: number;
  media_type?: MediaType;
  /** Short overview/description, when the metadata source provides one. */
  overview?: string;
  /** Whether this title is already monitored in a cellarr library. */
  already_added?: boolean;
}

/** The body the add call posts to create monitored content. */
export interface AddContentRequest {
  library_id: string;
  foreign_id: string;
  title: string;
  quality_profile_id?: string;
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

/** Free-text lookup for titles to add. */
export function lookup(term: string, signal?: AbortSignal): Promise<LookupResult[]> {
  return api.request<LookupResult[]>('/lookup', { query: { term }, signal });
}

/** Create monitored content from a chosen lookup result. */
export function addContent(body: AddContentRequest, signal?: AbortSignal) {
  return api.request<{ id: string }>('/content', { method: 'POST', body, signal });
}

/** Interactive/manual release search for a content node. */
export function searchReleases(
  contentId: string,
  signal?: AbortSignal
): Promise<CandidateRelease[]> {
  return api.request<CandidateRelease[]>('/releases', {
    query: { content: contentId },
    signal,
  });
}

/** Hand a chosen release to a download client (the manual grab). */
export function grabRelease(guid: string, contentId: string, signal?: AbortSignal) {
  return api.request<{ accepted: boolean }>('/releases/grab', {
    method: 'POST',
    body: { guid, content_id: contentId },
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
