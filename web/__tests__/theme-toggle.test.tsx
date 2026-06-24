import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/react';

import { ThemeProvider } from '@lib/ThemeProvider';
import { STORAGE_KEY } from '@lib/theme';
import ThemeBarToggle from '@app/_components/ThemeBarToggle';

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

describe('ThemeBarToggle (SRCL ActionButton segmented control)', () => {
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
    const { getByLabelText } = render(
      <ThemeProvider>
        <ThemeBarToggle />
      </ThemeProvider>
    );
    const darkSegment = getByLabelText('Dark theme');
    expect(darkSegment).toBeTruthy();
    fireEvent.click(darkSegment);
    expect(document.body.classList.contains('theme-dark')).toBe(true);
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe('dark');
  });

  it('renders the three System/Light/Dark segments', () => {
    const { getByLabelText } = render(
      <ThemeProvider>
        <ThemeBarToggle />
      </ThemeProvider>
    );
    expect(getByLabelText('System theme')).toBeTruthy();
    expect(getByLabelText('Light theme')).toBeTruthy();
    expect(getByLabelText('Dark theme')).toBeTruthy();
  });
});
