// Vitest global setup. jsdom 25 under Node 25 ships `window.localStorage` as an
// empty object with no Storage methods, so tests that call setItem/getItem/clear
// throw "is not a function". Install a minimal, spec-shaped Storage backed by a
// Map so the existing theme/persistence tests have a real localStorage to drive.
import { beforeEach } from 'vitest';
import { configure } from '@testing-library/dom';

// Give async queries (waitFor / findBy*) generous headroom. Several screens load
// their data through parallel async effects; the 1s default can be exceeded purely
// from CPU contention when the full suite runs across many fork workers, which
// surfaced as rare false-timeout flakes (not real bugs). A higher ceiling only
// affects tests that are genuinely waiting — passing queries still resolve fast.
configure({ asyncUtilTimeout: 8000 });

class MemoryStorage implements Storage {
  private store = new Map<string, string>();

  get length(): number {
    return this.store.size;
  }

  clear(): void {
    this.store.clear();
  }

  getItem(key: string): string | null {
    return this.store.has(key) ? (this.store.get(key) as string) : null;
  }

  key(index: number): string | null {
    return Array.from(this.store.keys())[index] ?? null;
  }

  removeItem(key: string): void {
    this.store.delete(key);
  }

  setItem(key: string, value: string): void {
    this.store.set(key, String(value));
  }
}

function ensureStorage(name: 'localStorage' | 'sessionStorage') {
  const existing = (window as unknown as Record<string, unknown>)[name] as
    | Storage
    | undefined;
  if (!existing || typeof existing.setItem !== 'function') {
    Object.defineProperty(window, name, {
      configurable: true,
      writable: true,
      value: new MemoryStorage(),
    });
  }
}

ensureStorage('localStorage');
ensureStorage('sessionStorage');

// Re-install before every test so a test that swaps the object out (or jsdom
// resetting state between files) still finds a working Storage.
beforeEach(() => {
  ensureStorage('localStorage');
  ensureStorage('sessionStorage');
});
