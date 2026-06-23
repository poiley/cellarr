import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/react';

import { ThemeProvider } from '@lib/ThemeProvider';
import { STORAGE_KEY } from '@lib/theme';
import ThemeToggle from '@app/_components/ThemeToggle';

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

describe('ThemeToggle (SRCL RadioButton group)', () => {
  beforeEach(() => {
    window.localStorage.clear();
    document.body.className = '';
    document.body.removeAttribute('style');
    installMatchMedia(false);
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('selecting Dark pins the dark theme and persists the choice', () => {
    const { container } = render(
      <ThemeProvider>
        <ThemeToggle />
      </ThemeProvider>
    );
    const darkRadio = container.querySelector(
      'input[name="cellarr-theme"][value="dark"]'
    ) as HTMLInputElement;
    expect(darkRadio).toBeTruthy();
    fireEvent.click(darkRadio);
    expect(document.body.classList.contains('theme-dark')).toBe(true);
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe('dark');
  });

  it('renders the three System/Light/Dark options', () => {
    const { container } = render(
      <ThemeProvider>
        <ThemeToggle />
      </ThemeProvider>
    );
    const radios = container.querySelectorAll('input[name="cellarr-theme"]');
    expect(radios.length).toBe(3);
    const values = Array.from(radios).map((r) => (r as HTMLInputElement).value).sort();
    expect(values).toEqual(['dark', 'light', 'system']);
  });
});
