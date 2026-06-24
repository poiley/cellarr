import { describe, expect, it } from 'vitest';

import {
  asDecisionRecord,
  asGlobalHistoryRow,
  asHistoryRecord,
  contentRefLabel,
  formatBytes,
  formatConfidence,
  formatHistoryDate,
  formatSigned,
  formatTimestamp,
  historyEventLabel,
  rejectReasonLabel,
  stageLabel,
  transitionKindLabel,
  transitionLabel,
  v3EventLabel,
  verdictSummary,
} from '@app/_lib/decisionlog';
import type { Score, Transition } from '@app/_lib/decisionlog';
import type { HistoryRecordV3 } from '@lib/api/types';

describe('decision-log domain helpers', () => {
  it('formats signed custom-format scores', () => {
    expect(formatSigned(0)).toBe('0');
    expect(formatSigned(50)).toBe('+50');
    expect(formatSigned(-25)).toBe('-25');
  });

  it('formats byte sizes into binary units', () => {
    expect(formatBytes(undefined)).toBe('—');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1024)).toBe('1.0 KiB');
    expect(formatBytes(5 * 1024 * 1024 * 1024)).toBe('5.0 GiB');
  });

  it('formats confidence as a percentage', () => {
    expect(formatConfidence(1)).toBe('100%');
    expect(formatConfidence(0.5)).toBe('50%');
    expect(formatConfidence(0)).toBe('0%');
  });

  it('labels stages and transitions', () => {
    expect(stageLabel('decide')).toBe('Decide');
    expect(stageLabel('held_for_review')).toBe('Held for review');
    const t: Transition = { from: 'decide', to: 'grab', kind: 'advance' };
    expect(transitionLabel(t)).toBe('Decide → Grab');
    expect(transitionKindLabel('reject')).toBe('reject');
  });

  it('labels history events', () => {
    expect(historyEventLabel('download_failed')).toBe('Download failed');
    expect(historyEventLabel('imported')).toBe('Imported');
  });

  it('summarizes a grab verdict with its score', () => {
    const score: Score = { quality_rank: 3, custom_format_score: 120 };
    expect(verdictSummary({ verdict: 'grab', score })).toContain('+120');
    expect(verdictSummary({ verdict: 'grab', score })).toContain('#3');
  });

  it('summarizes an upgrade verdict with the score delta', () => {
    const summary = verdictSummary({
      verdict: 'upgrade',
      replacing: 'file-1',
      from: { quality_rank: 2, custom_format_score: 10 },
      to: { quality_rank: 4, custom_format_score: 90 },
    });
    expect(summary).toContain('+10');
    expect(summary).toContain('+90');
  });

  it('summarizes a reject verdict with a friendly reason', () => {
    expect(verdictSummary({ verdict: 'reject', reason: { reason: 'not_an_upgrade' } })).toContain(
      'equal or better'
    );
    expect(
      rejectReasonLabel({ reason: 'other', detail: 'custom thing' })
    ).toContain('custom thing');
  });

  it('builds a short content-ref label', () => {
    const label = contentRefLabel({
      id: '0123456789abcdef',
      library_id: 'lib',
      media_type: 'tv',
      coords: null,
    });
    expect(label).toContain('tv');
    expect(label).toContain('01234567');
  });

  it('formats an ISO timestamp and passes through garbage', () => {
    expect(formatTimestamp('')).toBe('—');
    expect(formatTimestamp('not-a-date')).toBe('not-a-date');
    expect(formatTimestamp('2024-01-02T03:04:05Z')).not.toBe('—');
  });

  it('narrows a loose decision record', () => {
    const typed = asDecisionRecord({
      at: '2024-01-01T00:00:00Z',
      run_id: 'run-1',
      transition: { from: 'decide', to: 'rejected', kind: 'reject' },
      decision: {
        content_ref: { id: 'c1', library_id: 'l1', media_type: 'movie', coords: { type: 'movie' } },
        release: { indexer_id: 'i1', title: 'X', download_url: 'u', protocol: 'torrent' },
        verdict: { verdict: 'reject', reason: { reason: 'quality_not_allowed' } },
      },
    });
    expect(typed.run_id).toBe('run-1');
    expect(typed.transition.kind).toBe('reject');
    expect(typed.decision?.verdict.verdict).toBe('reject');
  });

  it('narrows a loose decision record missing a transition', () => {
    const typed = asDecisionRecord({ at: 'x', run_id: 'r' });
    expect(typed.transition.from).toBe('discover');
    expect(typed.decision).toBeUndefined();
  });

  it('narrows a loose history record', () => {
    const typed = asHistoryRecord({
      at: '2024-01-01T00:00:00Z',
      content_id: 'c1',
      run_id: 'run-9',
      event: { event: 'grabbed', grab_id: 'g1' },
    });
    expect(typed.content_id).toBe('c1');
    expect(typed.run_id).toBe('run-9');
    expect(typed.event.event).toBe('grabbed');
  });

  it('narrows a v3 global-feed history row', () => {
    // The shim serializes `date` as unix seconds (a number); the loose
    // HistoryRecordV3 type still declares it `string`, so cast for the literal.
    const row = asGlobalHistoryRow({
      id: 'node-1',
      eventType: 'grabbed',
      date: 1_700_000_000,
      sourceTitle: 'Blade Runner (1982)',
      data: { runId: 'run-7', contentId: 'node-1' },
    } as unknown as HistoryRecordV3);
    expect(row.eventType).toBe('grabbed');
    expect(row.sourceTitle).toBe('Blade Runner (1982)');
    expect(row.contentId).toBe('node-1');
    expect(row.runId).toBe('run-7');
    expect(row.date).toBe(1_700_000_000);
  });

  it('reads alternate run/content key spellings in a v3 row', () => {
    const row = asGlobalHistoryRow({
      eventType: 'downloadFolderImported',
      date: '1700000000',
      run_id: 'run-snake',
      content_id: 'node-snake',
    });
    expect(row.runId).toBe('run-snake');
    expect(row.contentId).toBe('node-snake');
    expect(row.date).toBe('1700000000');
  });

  it('labels v3 event types, humanizing unknown ones', () => {
    expect(v3EventLabel('grabbed')).toBe('Grabbed');
    expect(v3EventLabel('downloadFolderImported')).toBe('Imported');
    expect(v3EventLabel('movieFileDeleted')).toBe('Deleted');
    expect(v3EventLabel('someNewEvent')).toBe('Some New Event');
    expect(v3EventLabel('snake_case_thing')).toBe('Snake case thing');
  });

  it('formats history dates from unix seconds and ISO alike', () => {
    expect(formatHistoryDate('')).toBe('—');
    expect(formatHistoryDate(0)).toBe('—');
    expect(formatHistoryDate('not-a-date')).toBe('not-a-date');
    // A real unix-seconds value and its string form resolve to the same instant.
    expect(formatHistoryDate(1_700_000_000)).toBe(formatHistoryDate('1700000000'));
    expect(formatHistoryDate('2024-01-02T03:04:05Z')).not.toBe('—');
  });
});
