import { describe, expect, it } from 'vitest';

import {
  formatCountdown,
  formatIso,
  humanizeDuration,
  lastStatusGlyph,
} from '@app/_lib/activity';

describe('activity helpers', () => {
  const now = Date.parse('2026-06-24T12:00:00.000Z');

  describe('formatCountdown', () => {
    it('returns a dash for missing/invalid input', () => {
      expect(formatCountdown(undefined, now)).toBe('—');
      expect(formatCountdown(null, now)).toBe('—');
      expect(formatCountdown('not-a-date', now)).toBe('—');
    });

    it('counts forward to a future run', () => {
      const future = new Date(now + 4 * 60000 + 12 * 1000).toISOString();
      expect(formatCountdown(future, now)).toBe('in 4m 12s');
    });

    it('counts up overdue for a past run', () => {
      const past = new Date(now - 30 * 1000).toISOString();
      expect(formatCountdown(past, now)).toBe('overdue 30s');
    });

    it('says due now within a second', () => {
      expect(formatCountdown(new Date(now).toISOString(), now)).toBe('due now');
    });
  });

  describe('humanizeDuration', () => {
    it('compacts hours/minutes and drops seconds when hours present', () => {
      expect(humanizeDuration(3 * 3600 + 2 * 60 + 5)).toBe('3h 2m');
    });
    it('shows minutes and seconds under an hour', () => {
      expect(humanizeDuration(4 * 60 + 12)).toBe('4m 12s');
    });
    it('shows bare seconds', () => {
      expect(humanizeDuration(30)).toBe('30s');
      expect(humanizeDuration(0)).toBe('0s');
    });
  });

  describe('formatIso', () => {
    it('returns a dash for null/invalid', () => {
      expect(formatIso(null)).toBe('—');
      expect(formatIso('nope')).toBe('—');
    });
    it('formats a real timestamp', () => {
      expect(formatIso(new Date(now).toISOString())).not.toBe('—');
    });
  });

  describe('lastStatusGlyph', () => {
    it('marks never-run for empty status', () => {
      expect(lastStatusGlyph(undefined)).toEqual({ glyph: '●', label: 'never run' });
    });
    it('marks success-ish with a check', () => {
      expect(lastStatusGlyph('completed').glyph).toBe('✓');
      expect(lastStatusGlyph('ok').glyph).toBe('✓');
    });
    it('marks failure-ish with a cross', () => {
      expect(lastStatusGlyph('failed').glyph).toBe('✗');
      expect(lastStatusGlyph('error').glyph).toBe('✗');
    });
    it('falls back to a dot for anything else', () => {
      expect(lastStatusGlyph('running').glyph).toBe('●');
    });
  });
});
