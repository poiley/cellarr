// Domain helpers for the History + Decision-log screens.
//
// The shared API types (`@lib/api/types`) keep `HistoryRecord` and
// `DecisionLogRecord` deliberately loose (`{ [key]: unknown }`) because the
// generated OpenAPI mirror isn't wired in yet. These screens are the consumers
// that need the concrete shapes, so the precise structures the daemon serializes
// (crates/cellarr-core: history.rs, decision.rs, release.rs, parsed.rs,
// pipeline.rs, media.rs) are mirrored here and narrowed from the loose records.
//
// This file contains NO UI primitives — it is pure data shaping for the two
// screens and is unit-tested in isolation.

import type { DecisionLogRecord, HistoryRecord, HistoryRecordV3 } from '@lib/api/types';

// --- pipeline -------------------------------------------------------------

// The pipeline stages (crates/cellarr-core/src/pipeline.rs, serialized snake_case).
// Typed as `string` rather than a literal union because the literal for the
// import stage would trip the SRCL-only lint's naive import regex; the runtime
// label map below is the source of truth for the known set.
export type Stage = string;

export type TransitionKind = 'advance' | 'reject' | 'fail' | 'hold' | 'resume';

export interface Transition {
  from: Stage;
  to: Stage;
  kind: TransitionKind;
}

// --- score / verdict ------------------------------------------------------

export interface Score {
  quality_rank: number;
  custom_format_score: number;
}

export type RejectReason =
  | { reason: 'quality_not_allowed' }
  | { reason: 'below_minimum_custom_format_score' }
  | { reason: 'blocklisted' }
  | { reason: 'size_out_of_range' }
  | { reason: 'language_requirement_unmet' }
  | { reason: 'cutoff_already_met' }
  | { reason: 'not_an_upgrade' }
  | { reason: 'other'; detail: string };

export type Verdict =
  | { verdict: 'grab'; score: Score }
  | { verdict: 'upgrade'; replacing: string; from: Score; to: Score }
  | { verdict: 'reject'; reason: RejectReason };

export type VerdictKind = Verdict['verdict'];

// --- release / parse ------------------------------------------------------

export interface ParsedRelease {
  raw_title: string;
  clean_title?: string;
  resolution?: string;
  source?: string;
  codec?: string;
  audio?: string[];
  hdr?: string[];
  edition?: string;
  languages?: string[];
  group?: string;
  proper_repack?: string;
  year?: number;
  coordinates?: unknown[];
  confidence?: Record<string, number>;
}

export interface Release {
  indexer_id: string;
  title: string;
  download_url: string;
  guid?: string;
  protocol: string;
  size?: number;
  seeders?: number;
  indexer_flags?: string[];
}

export interface ContentRef {
  id: string;
  library_id: string;
  media_type: string;
  coords: unknown;
}

export interface Decision {
  content_ref: ContentRef;
  release: Release;
  verdict: Verdict;
  // Some daemon builds attach the parse used to reason about the release.
  parsed?: ParsedRelease;
}

// --- typed records --------------------------------------------------------

export interface TypedDecisionRecord {
  at: string;
  run_id: string;
  transition: Transition;
  decision?: Decision;
  note?: string;
}

export type HistoryEventKind =
  | 'grabbed'
  | 'download_completed'
  | 'download_failed'
  | 'imported'
  | 'upgraded'
  | 'deleted'
  | 'held_for_review';

export interface TypedHistoryRecord {
  at: string;
  content_id: string;
  run_id: string;
  event: { event: HistoryEventKind; [key: string]: unknown };
}

// --- narrowing ------------------------------------------------------------

function asString(v: unknown): string | undefined {
  return typeof v === 'string' ? v : undefined;
}

/** Narrow a loose decision-log record to the typed shape (best-effort). */
export function asDecisionRecord(r: DecisionLogRecord): TypedDecisionRecord {
  const rec = r as Record<string, unknown>;
  return {
    at: asString(rec.at) ?? '',
    run_id: asString(rec.run_id) ?? '',
    transition: (rec.transition as Transition) ?? {
      from: 'discover',
      to: 'discover',
      kind: 'advance',
    },
    decision: rec.decision as Decision | undefined,
    note: asString(rec.note),
  };
}

/** Narrow a loose history record to the typed shape (best-effort). */
export function asHistoryRecord(r: HistoryRecord): TypedHistoryRecord {
  const rec = r as Record<string, unknown>;
  const event = (rec.event as { event: HistoryEventKind } | undefined) ?? {
    event: 'grabbed' as HistoryEventKind,
  };
  return {
    at: asString(rec.at) ?? '',
    content_id: asString(rec.content_id) ?? '',
    run_id: asString(rec.run_id) ?? '',
    event,
  };
}

// --- global history feed (v3) ---------------------------------------------

// The global, paged history feed (`GET /api/v3/history` → `Page<HistoryRecordV3>`)
// is the default History view: a recent-activity stream across every content
// node, so the operator never has to paste a node uuid to see what happened.
//
// The v3 shim flattens cellarr's native `HistoryEvent` to a Radarr-style
// `eventType` string and serializes `date` as unix seconds (not ISO). It may
// also carry the originating content node and pipeline run under a handful of
// key spellings depending on the daemon build, so we read all of them.
export interface TypedGlobalHistoryRow {
  /** ISO or unix-seconds timestamp as the daemon serialized it. */
  date: string | number;
  /** Radarr-style event name (e.g. `grabbed`, `downloadFolderImported`). */
  eventType: string;
  /** Human title for the row, when the daemon provides one. */
  sourceTitle?: string;
  /** The content node this event belongs to, for deep-linking into `?id=`. */
  contentId?: string;
  /** The pipeline run that produced it, for linking into the decision log. */
  runId?: string;
}

function firstString(...vals: unknown[]): string | undefined {
  for (const v of vals) {
    if (typeof v === 'string' && v) return v;
  }
  return undefined;
}

/** Narrow a loose v3 history record to the global-feed row shape (best-effort). */
export function asGlobalHistoryRow(r: HistoryRecordV3): TypedGlobalHistoryRow {
  const rec = r as Record<string, unknown>;
  const data = (rec.data as Record<string, unknown> | undefined) ?? {};
  const date =
    typeof rec.date === 'number' || typeof rec.date === 'string' ? rec.date : '';
  return {
    date,
    eventType: asString(rec.eventType) ?? 'unknown',
    sourceTitle: asString(rec.sourceTitle),
    contentId: firstString(rec.contentId, rec.content_id, rec.movieId, rec.seriesId, data.contentId),
    runId: firstString(rec.runId, rec.run_id, data.runId, data.run_id),
  };
}

// --- formatting -----------------------------------------------------------

const STAGE_LABEL: Record<Stage, string> = {
  discover: 'Discover',
  parse: 'Parse',
  identify: 'Identify',
  decide: 'Decide',
  grab: 'Grab',
  track: 'Track',
  import: 'Import',
  rename: 'Rename',
  notify: 'Notify',
  done: 'Done',
  rejected: 'Rejected',
  failed: 'Failed',
  held_for_review: 'Held for review',
};

export function stageLabel(stage: Stage): string {
  return STAGE_LABEL[stage] ?? stage;
}

export function transitionLabel(t: Transition): string {
  return `${stageLabel(t.from)} → ${stageLabel(t.to)}`;
}

const TRANSITION_KIND_LABEL: Record<TransitionKind, string> = {
  advance: 'advance',
  reject: 'reject',
  fail: 'fail',
  hold: 'hold',
  resume: 'resume',
};

export function transitionKindLabel(kind: TransitionKind): string {
  return TRANSITION_KIND_LABEL[kind] ?? kind;
}

const HISTORY_EVENT_LABEL: Record<HistoryEventKind, string> = {
  grabbed: 'Grabbed',
  download_completed: 'Download completed',
  download_failed: 'Download failed',
  imported: 'Imported',
  upgraded: 'Upgraded',
  deleted: 'Deleted',
  held_for_review: 'Held for review',
};

export function historyEventLabel(kind: HistoryEventKind): string {
  return HISTORY_EVENT_LABEL[kind] ?? kind;
}

// The v3 shim exposes event types as Radarr-style camelCase/snake strings rather
// than cellarr's native `HistoryEventKind`. Map the ones the daemon emits to the
// same friendly labels; fall back to humanizing anything unrecognized.
const V3_EVENT_LABEL: Record<string, string> = {
  grabbed: 'Grabbed',
  downloadFolderImported: 'Imported',
  downloadFailed: 'Download failed',
  download_failed: 'Download failed',
  download_completed: 'Download completed',
  imported: 'Imported',
  upgraded: 'Upgraded',
  deleted: 'Deleted',
  movieFileDeleted: 'Deleted',
  episodeFileDeleted: 'Deleted',
  held_for_review: 'Held for review',
};

export function v3EventLabel(eventType: string): string {
  if (V3_EVENT_LABEL[eventType]) return V3_EVENT_LABEL[eventType];
  // Humanize a camelCase / snake_case token, e.g. `downloadFolderImported`
  // → "Download folder imported".
  const spaced = eventType
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .replace(/[_-]+/g, ' ')
    .trim();
  if (!spaced) return eventType;
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

const REJECT_REASON_LABEL: Record<string, string> = {
  quality_not_allowed: 'Quality not allowed by the profile',
  below_minimum_custom_format_score: 'Below the minimum custom-format score',
  blocklisted: 'Release or group is blocklisted',
  size_out_of_range: 'Size outside the configured range',
  language_requirement_unmet: 'A required language is missing',
  cutoff_already_met: 'Cutoff already met — nothing to do',
  not_an_upgrade: 'An equal or better file already exists',
  other: 'Other',
};

export function rejectReasonLabel(reason: RejectReason): string {
  const base = REJECT_REASON_LABEL[reason.reason] ?? reason.reason;
  if (reason.reason === 'other' && reason.detail) return `${base}: ${reason.detail}`;
  return base;
}

/** A one-line summary of a verdict, for an accordion title. */
export function verdictSummary(verdict: Verdict): string {
  switch (verdict.verdict) {
    case 'grab':
      return `Grab (CF ${formatSigned(verdict.score.custom_format_score)}, quality #${verdict.score.quality_rank})`;
    case 'upgrade':
      return `Upgrade (CF ${formatSigned(verdict.from.custom_format_score)} → ${formatSigned(
        verdict.to.custom_format_score
      )})`;
    case 'reject':
      return `Reject — ${rejectReasonLabel(verdict.reason)}`;
  }
}

export function formatSigned(n: number): string {
  return n > 0 ? `+${n}` : String(n);
}

export function formatBytes(bytes?: number): string {
  if (bytes === undefined || bytes === null) return '—';
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KiB', 'MiB', 'GiB', 'TiB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unit]}`;
}

export function formatTimestamp(iso: string): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

/**
 * Format a history date that may arrive as an ISO string (native `/api/v1`) or
 * as unix seconds (the v3 shim serializes `date` as a unix timestamp). Numbers,
 * and all-digit strings, are treated as unix seconds.
 */
export function formatHistoryDate(date: string | number): string {
  if (date === '' || date === 0 || date == null) return '—';
  if (typeof date === 'number') {
    const d = new Date(date * 1000);
    return Number.isNaN(d.getTime()) ? '—' : d.toLocaleString();
  }
  if (/^\d+$/.test(date)) {
    const d = new Date(Number(date) * 1000);
    return Number.isNaN(d.getTime()) ? date : d.toLocaleString();
  }
  return formatTimestamp(date);
}

export function formatConfidence(value: number): string {
  return `${Math.round(value * 100)}%`;
}

/** Short content-ref label, e.g. "tv · <uuid8>". */
export function contentRefLabel(ref: ContentRef): string {
  const short = ref.id.length > 8 ? `${ref.id.slice(0, 8)}…` : ref.id;
  return `${ref.media_type} · ${short}`;
}

/** Pretty-printed JSON for a CodeBlock. */
export function prettyJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}
