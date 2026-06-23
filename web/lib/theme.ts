// Theme controller — the only allowed non-SRCL, non-API "glue" module.
//
// It introduces NO UI primitives. It only selects SRCL's own `body.theme-light` /
// `body.theme-dark` classes (docs/10-ui.md §Theming) from:
//   1. a persisted System/Light/Dark choice (localStorage), and
//   2. the OS preference via `window.matchMedia('(prefers-color-scheme: dark)')`
// while on "System", following live `change` events; and it sets the CSS
// `color-scheme` property so native form controls / scrollbars match.

export type ThemeChoice = 'system' | 'light' | 'dark';
export type ResolvedTheme = 'light' | 'dark';

export const STORAGE_KEY = 'cellarr-theme';
const LIGHT_CLASS = 'theme-light';
const DARK_CLASS = 'theme-dark';
const PREFERS_DARK = '(prefers-color-scheme: dark)';

/** The stored choice, defaulting to "system" when absent or invalid. */
export function readStoredChoice(): ThemeChoice {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw === 'light' || raw === 'dark' || raw === 'system') return raw;
  } catch {
    // localStorage may be unavailable (private mode / SSR) — fall through.
  }
  return 'system';
}

/** Persist a choice. Swallows storage failures (private mode). */
export function writeStoredChoice(choice: ThemeChoice): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, choice);
  } catch {
    // ignore
  }
}

/** The OS preference right now. */
export function systemPrefersDark(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia(PREFERS_DARK).matches
  );
}

/** Resolve a choice to a concrete light/dark theme. */
export function resolveTheme(choice: ThemeChoice): ResolvedTheme {
  if (choice === 'light') return 'light';
  if (choice === 'dark') return 'dark';
  return systemPrefersDark() ? 'dark' : 'light';
}

/**
 * Apply a resolved theme to <body> and set `color-scheme`. Removes the opposite
 * SRCL class first so the body never carries both. Safe to call repeatedly.
 */
export function applyTheme(resolved: ResolvedTheme): void {
  if (typeof document === 'undefined') return;
  const body = document.body;
  if (!body) return;
  body.classList.remove(LIGHT_CLASS, DARK_CLASS);
  body.classList.add(resolved === 'dark' ? DARK_CLASS : LIGHT_CLASS);
  body.style.colorScheme = resolved;
}

type Unsubscribe = () => void;

/**
 * Subscribe to OS preference changes. The handler fires whenever the OS toggles
 * day/night; callers use this only while the choice is "system".
 */
export function subscribeSystem(handler: (prefersDark: boolean) => void): Unsubscribe {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') {
    return () => {};
  }
  const mql = window.matchMedia(PREFERS_DARK);
  const listener = (event: MediaQueryListEvent) => handler(event.matches);
  // Older Safari only has addListener/removeListener.
  if (typeof mql.addEventListener === 'function') {
    mql.addEventListener('change', listener);
    return () => mql.removeEventListener('change', listener);
  }
  mql.addListener(listener);
  return () => mql.removeListener(listener);
}

/**
 * The inline pre-hydration script source. Injected into <head> so the correct
 * `body.theme-*` class + `color-scheme` are set before first paint (no flash).
 * Kept as a single self-contained string with no closure over module scope.
 */
export const PREHYDRATION_SCRIPT = `(function(){try{
  var k=${JSON.stringify(STORAGE_KEY)};
  var c=null;try{c=localStorage.getItem(k);}catch(e){}
  if(c!=='light'&&c!=='dark'&&c!=='system')c='system';
  var dark=c==='dark'||(c==='system'&&window.matchMedia&&window.matchMedia('${PREFERS_DARK}').matches);
  var b=document.body;
  b.classList.remove('${LIGHT_CLASS}','${DARK_CLASS}');
  b.classList.add(dark?'${DARK_CLASS}':'${LIGHT_CLASS}');
  b.style.colorScheme=dark?'dark':'light';
}catch(e){}})();`;
