import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { ThemeProvider } from '@lib/ThemeProvider';
import { ModalProvider } from '@components/page/ModalContext';
import { HotkeysProvider } from '@modules/hotkeys';

const request = vi.fn();
vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: { request: (...args: unknown[]) => request(...args) },
  };
});

// useSearchParams needs a router context; mock next/navigation.
let searchValue = '';
vi.mock('next/navigation', () => ({
  useSearchParams: () => new URLSearchParams(searchValue),
}));

import InteractivePage from '@app/interactive/page';

function installMatchMedia() {
  window.matchMedia = vi.fn().mockReturnValue({
    matches: false,
    media: '(prefers-color-scheme: dark)',
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    onchange: null,
    dispatchEvent: () => true,
  }) as unknown as typeof window.matchMedia;
}

function renderPage() {
  return render(
    <ThemeProvider>
      <HotkeysProvider>
        <ModalProvider>
          <InteractivePage />
        </ModalProvider>
      </HotkeysProvider>
    </ThemeProvider>
  );
}

describe('Interactive / manual-search screen', () => {
  beforeEach(() => {
    request.mockReset();
    searchValue = '';
    window.localStorage.clear();
    document.body.className = '';
    installMatchMedia();
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('shows the idle prompt with no content id', () => {
    renderPage();
    expect(screen.getByText(/Enter a content id/i)).toBeTruthy();
  });

  it('auto-searches when arriving with ?content= and shows quality + score badges', async () => {
    searchValue = 'content=c1';
    request.mockResolvedValue([
      {
        guid: 'g1',
        title: 'Some.Movie.2024.Bluray-1080p',
        indexer: 'demo',
        protocol: 'torrent',
        quality: 'Bluray-1080p',
        cf_score: 120,
        size: 8_589_934_592,
        seeders: 42,
      },
    ]);
    renderPage();

    await waitFor(() =>
      expect(request).toHaveBeenCalledWith('/releases', expect.objectContaining({ query: { content: 'c1' } }))
    );
    await waitFor(() => expect(screen.getByText('Bluray-1080p')).toBeTruthy());
    expect(screen.getByText('+120')).toBeTruthy();
    expect(screen.getByText('8.0 GB')).toBeTruthy();
    expect(screen.getByText('42')).toBeTruthy();
  });

  it('grabs a release through the command endpoint', async () => {
    searchValue = 'content=c1';
    request
      .mockResolvedValueOnce([{ guid: 'g1', title: 'rel', quality: 'WEBDL-720p', cf_score: 0 }])
      .mockResolvedValueOnce({ accepted: true });
    renderPage();

    // Two "Grab" texts exist: the table header column and the action button.
    // Click the one inside the ActionButton (role="button").
    const grabButton = await waitFor(() => {
      const btn = screen
        .getAllByRole('button')
        .find((el) => el.textContent?.includes('Grab'));
      if (!btn) throw new Error('grab button not found yet');
      return btn;
    });
    fireEvent.click(grabButton);

    await waitFor(() =>
      expect(request).toHaveBeenCalledWith('/releases/grab', expect.objectContaining({ method: 'POST' }))
    );
    await waitFor(() => expect(screen.getByText('grabbed')).toBeTruthy());
  });

  it('renders an empty state when no releases are found', async () => {
    searchValue = 'content=c1';
    request.mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByText(/No candidate releases/i)).toBeTruthy());
  });

  it('renders an error banner when the release search fails', async () => {
    searchValue = 'content=c1';
    request.mockRejectedValue(new Error('nope'));
    renderPage();
    await waitFor(() => expect(screen.getByText(/Release search failed/i)).toBeTruthy());
  });
});
