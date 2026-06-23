import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  STORAGE_KEY,
  applyTheme,
  readStoredChoice,
  resolveTheme,
  subscribeSystem,
} from '@lib/theme';

// A controllable matchMedia mock: lets a test flip the OS preference and fire
// the 'change' event the controller subscribes to.
type Listener = (e: { matches: boolean }) => void;

function installMatchMedia(initialDark: boolean) {
  let matches = initialDark;
  const listeners = new Set<Listener>();
  const mql = {
    get matches() {
      return matches;
    },
    media: '(prefers-color-scheme: dark)',
    addEventListener: (_: string, cb: Listener) => listeners.add(cb),
    removeEventListener: (_: string, cb: Listener) => listeners.delete(cb),
    addListener: (cb: Listener) => listeners.add(cb),
    removeListener: (cb: Listener) => listeners.delete(cb),
    onchange: null,
    dispatchEvent: () => true,
  };
  window.matchMedia = vi.fn().mockReturnValue(mql) as unknown as typeof window.matchMedia;
  return {
    setDark(next: boolean) {
      matches = next;
      for (const cb of listeners) cb({ matches });
    },
  };
}

describe('theme controller', () => {
  beforeEach(() => {
    window.localStorage.clear();
    document.body.className = '';
    document.body.removeAttribute('style');
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('resolves System to the prefers-color-scheme class (dark)', () => {
    installMatchMedia(true);
    expect(readStoredChoice()).toBe('system');
    const resolved = resolveTheme('system');
    expect(resolved).toBe('dark');
    applyTheme(resolved);
    expect(document.body.classList.contains('theme-dark')).toBe(true);
    expect(document.body.classList.contains('theme-light')).toBe(false);
  });

  it('resolves System to the prefers-color-scheme class (light)', () => {
    installMatchMedia(false);
    const resolved = resolveTheme('system');
    expect(resolved).toBe('light');
    applyTheme(resolved);
    expect(document.body.classList.contains('theme-light')).toBe(true);
  });

  it('reacts to a matchMedia change while on System', () => {
    const mm = installMatchMedia(false);
    // Wire the subscription the way the controller does while on "System".
    const unsubscribe = subscribeSystem((prefersDark) => {
      applyTheme(prefersDark ? 'dark' : 'light');
    });
    applyTheme(resolveTheme('system')); // initial: light
    expect(document.body.classList.contains('theme-light')).toBe(true);

    mm.setDark(true); // OS flips to dark
    expect(document.body.classList.contains('theme-dark')).toBe(true);
    expect(document.body.classList.contains('theme-light')).toBe(false);

    mm.setDark(false); // OS flips back to light
    expect(document.body.classList.contains('theme-light')).toBe(true);
    unsubscribe();
  });

  it('honors a persisted Light override regardless of OS preference', () => {
    installMatchMedia(true); // OS says dark
    window.localStorage.setItem(STORAGE_KEY, 'light');
    expect(readStoredChoice()).toBe('light');
    const resolved = resolveTheme(readStoredChoice());
    expect(resolved).toBe('light');
    applyTheme(resolved);
    expect(document.body.classList.contains('theme-light')).toBe(true);
  });

  it('honors a persisted Dark override regardless of OS preference', () => {
    installMatchMedia(false); // OS says light
    window.localStorage.setItem(STORAGE_KEY, 'dark');
    expect(readStoredChoice()).toBe('dark');
    const resolved = resolveTheme(readStoredChoice());
    expect(resolved).toBe('dark');
    applyTheme(resolved);
    expect(document.body.classList.contains('theme-dark')).toBe(true);
  });

  it('sets the CSS color-scheme to match the resolved theme', () => {
    installMatchMedia(false);
    applyTheme('dark');
    expect(document.body.style.colorScheme).toBe('dark');
    applyTheme('light');
    expect(document.body.style.colorScheme).toBe('light');
  });
});
