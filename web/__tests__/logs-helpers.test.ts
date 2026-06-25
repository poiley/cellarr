import { describe, expect, it } from 'vitest';

import {
  parseLogLines,
  filterByLevel,
  detectLevel,
} from '@app/logs/_lib/logs';

describe('logs helpers', () => {
  it('detects the common level tokens (incl. aliases)', () => {
    expect(detectLevel('2026-06-20 INFO booting')).toBe('INFO');
    expect(detectLevel('[WARN] careful')).toBe('WARN');
    expect(detectLevel('WARNING legacy alias')).toBe('WARN');
    expect(detectLevel('ERR short alias')).toBe('ERROR');
    expect(detectLevel('   continuation line, no level')).toBeNull();
  });

  it('parses text into indexed lines and drops a single trailing newline', () => {
    const lines = parseLogLines('a INFO\nb ERROR\n');
    expect(lines.map((l) => l.text)).toEqual(['a INFO', 'b ERROR']);
    expect(lines[0].index).toBe(0);
    expect(lines[1].level).toBe('ERROR');
  });

  it('returns everything for the null (All) filter', () => {
    const lines = parseLogLines('a INFO\nb WARN\nc ERROR');
    expect(filterByLevel(lines, null)).toHaveLength(3);
  });

  it('keeps lines at or above the threshold, plus untagged context lines', () => {
    const lines = parseLogLines(
      ['a INFO hello', 'b WARN watch out', '    stack frame', 'c ERROR boom'].join('\n')
    );
    const errs = filterByLevel(lines, 'ERROR');
    // ERROR line + the untagged stack frame are kept; INFO/WARN dropped.
    expect(errs.map((l) => l.text)).toEqual(['    stack frame', 'c ERROR boom']);

    const warns = filterByLevel(lines, 'WARN');
    expect(warns.map((l) => l.text)).toEqual([
      'b WARN watch out',
      '    stack frame',
      'c ERROR boom',
    ]);
  });
});
