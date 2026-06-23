import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { PREHYDRATION_SCRIPT, STORAGE_KEY } from '@lib/theme';

// The pre-hydration script is injected into <head> and runs before first paint.
// Executing its source directly (as the browser would) must set the initial
// body class + color-scheme from the stored choice / OS preference — proving the
// no-flash guarantee.

function installMatchMedia(dark: boolean) {
  window.matchMedia = vi.fn().mockReturnValue({
    matches: dark,
    media: '(prefers-color-scheme: dark)',
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    onchange: null,
    dispatchEvent: () => true,
  }) as unknown as typeof window.matchMedia;
}

function runScript() {
  // The script is an IIFE string; eval it in the test's window/document scope.
  // eslint-disable-next-line no-eval
  (0, eval)(PREHYDRATION_SCRIPT);
}

describe('pre-hydration no-flash script', () => {
  beforeEach(() => {
    window.localStorage.clear();
    document.body.className = '';
    document.body.removeAttribute('style');
  });
  afterEach(() => vi.restoreAllMocks());

  it('sets theme-dark before paint when OS prefers dark and no override', () => {
    installMatchMedia(true);
    runScript();
    expect(document.body.classList.contains('theme-dark')).toBe(true);
    expect(document.body.style.colorScheme).toBe('dark');
  });

  it('sets theme-light before paint when OS prefers light', () => {
    installMatchMedia(false);
    runScript();
    expect(document.body.classList.contains('theme-light')).toBe(true);
    expect(document.body.style.colorScheme).toBe('light');
  });

  it('honors a persisted override before paint', () => {
    installMatchMedia(true); // OS dark
    window.localStorage.setItem(STORAGE_KEY, 'light');
    runScript();
    expect(document.body.classList.contains('theme-light')).toBe(true);
    expect(document.body.style.colorScheme).toBe('light');
  });
});
