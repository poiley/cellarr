import { describe, expect, it } from 'vitest';

import {
  buildQualityNameMap,
  isPlaceholderName,
  resolveQualityName,
} from '@app/_lib/quality';
import type { QualityDefinition } from '@lib/api/types';

// Mirrors the real GET /api/v3/qualitydefinition shape: human names keyed by
// quality.id (id 20 -> WEBDL-1080p, id 21 -> Bluray-1080p).
const DEFS: QualityDefinition[] = [
  { id: 20, title: 'WEBDL-1080p', weight: 20, minSize: 0, maxSize: null, preferredSize: null, quality: { id: 20, name: 'WEBDL-1080p', resolution: 1080, source: 'web' } },
  { id: 21, title: 'Bluray-1080p', weight: 21, minSize: 0, maxSize: null, preferredSize: null, quality: { id: 21, name: 'Bluray-1080p', resolution: 1080, source: 'bluray' } },
];

describe('isPlaceholderName', () => {
  it('matches the rank-N placeholder the daemon emits', () => {
    expect(isPlaceholderName('rank-20')).toBe(true);
    expect(isPlaceholderName('RANK-21')).toBe(true);
    expect(isPlaceholderName(' rank-5 ')).toBe(true);
  });
  it('does not match real quality names', () => {
    expect(isPlaceholderName('Bluray-1080p')).toBe(false);
    expect(isPlaceholderName('WEBDL-480p')).toBe(false);
    expect(isPlaceholderName('rank')).toBe(false);
  });
});

describe('buildQualityNameMap', () => {
  it('maps quality id -> human name', () => {
    const map = buildQualityNameMap(DEFS);
    expect(map.get('20')).toBe('WEBDL-1080p');
    expect(map.get('21')).toBe('Bluray-1080p');
  });

  it('skips placeholder names in favour of the title fallback', () => {
    const map = buildQualityNameMap([
      { id: 20, title: 'WEBDL-1080p', weight: 20, minSize: 0, maxSize: null, preferredSize: null, quality: { id: 20, name: 'rank-20', resolution: 0, source: 'unknown' } },
    ]);
    expect(map.get('20')).toBe('WEBDL-1080p');
  });

  it('returns an empty map for missing/invalid input', () => {
    expect(buildQualityNameMap(undefined).size).toBe(0);
    expect(buildQualityNameMap([] as QualityDefinition[]).size).toBe(0);
  });
});

describe('resolveQualityName', () => {
  it('prefers the definition map name over a placeholder on the profile item', () => {
    const map = buildQualityNameMap(DEFS);
    // The profile carries the unhelpful "rank-20" placeholder; the map wins.
    expect(resolveQualityName('20', 'rank-20', map)).toBe('WEBDL-1080p');
    expect(resolveQualityName('21', 'rank-21', map)).toBe('Bluray-1080p');
  });

  it('falls back to a non-placeholder profile name when the map lacks the id', () => {
    const map = buildQualityNameMap(DEFS);
    expect(resolveQualityName('99', 'Remux-2160p', map)).toBe('Remux-2160p');
  });

  it('avoids surfacing a bare rank-N placeholder when nothing resolves', () => {
    const map = buildQualityNameMap(DEFS);
    expect(resolveQualityName('99', 'rank-99', map)).toBe('Quality 99');
    expect(resolveQualityName('99', undefined, map)).toBe('Quality 99');
  });
});
