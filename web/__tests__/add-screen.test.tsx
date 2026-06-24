import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { ThemeProvider } from '@lib/ThemeProvider';
import { ModalProvider } from '@components/page/ModalContext';
import { HotkeysProvider } from '@modules/hotkeys';

// The Add screen talks to the shared client's v3 helpers + library/root-folder
// readers; mock them so the component test is hermetic.
const requestV3 = vi.fn();
const listLibraries = vi.fn();
const listRootFolders = vi.fn();
vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      requestV3: (...args: unknown[]) => requestV3(...args),
      listLibraries: (...args: unknown[]) => listLibraries(...args),
      listRootFolders: (...args: unknown[]) => listRootFolders(...args),
    },
  };
});

import AddPage from '@app/add/page';

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
          <AddPage />
        </ModalProvider>
      </HotkeysProvider>
    </ThemeProvider>
  );
}

describe('Add / search-new screen', () => {
  beforeEach(() => {
    requestV3.mockReset();
    listLibraries.mockReset();
    listRootFolders.mockReset();
    listLibraries.mockResolvedValue([
      {
        id: 'lib-movie',
        media_type: 'movie',
        name: 'Movies',
        root_folders: ['/movies'],
        default_quality_profile: 'qp-1',
      },
    ]);
    listRootFolders.mockResolvedValue([{ id: 1, path: '/movies' }]);
    window.localStorage.clear();
    document.body.className = '';
    installMatchMedia();
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('shows the idle prompt before any search', () => {
    renderPage();
    expect(screen.getByText(/Start typing above/i)).toBeTruthy();
  });

  // A raw v3 movie/series lookup candidate.
  const bladeRunner = {
    title: 'Blade Runner',
    titleSlug: 'blade-runner',
    year: 1982,
    tmdbId: 78,
    overview: 'A blade runner.',
    monitored: false,
    hasFile: false,
    status: 'released',
  };

  it('looks up titles on input (both movie + series) and renders a results table', async () => {
    requestV3.mockImplementation((path: string) => {
      if (path === '/movie/lookup') return Promise.resolve([bladeRunner]);
      if (path === '/series/lookup') return Promise.resolve([]);
      return Promise.resolve(undefined);
    });
    const { container } = renderPage();

    const input = container.querySelector('input[name="add-search"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'blade' } });

    // Wait out the real debounce + fetch — both lookup surfaces are queried.
    await waitFor(
      () =>
        expect(requestV3).toHaveBeenCalledWith(
          '/movie/lookup',
          expect.objectContaining({ query: { term: 'blade' } })
        ),
      { timeout: 2000 }
    );
    expect(requestV3).toHaveBeenCalledWith(
      '/series/lookup',
      expect.objectContaining({ query: { term: 'blade' } })
    );
    await waitFor(() => expect(screen.getByText('Blade Runner')).toBeTruthy());
    expect(screen.getByText('1982')).toBeTruthy();
  });

  it('opens a confirm dialog and posts the movie add on confirm', async () => {
    requestV3.mockImplementation((path: string) => {
      if (path === '/movie/lookup') return Promise.resolve([bladeRunner]);
      if (path === '/series/lookup') return Promise.resolve([]);
      if (path === '/movie') return Promise.resolve({ id: 'c1' });
      return Promise.resolve(undefined);
    });
    const { container } = renderPage();

    const input = container.querySelector('input[name="add-search"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'blade' } });
    await waitFor(() => expect(screen.getByText('Blade Runner')).toBeTruthy(), { timeout: 2000 });

    // Click the per-row Add ActionButton (avoid the "Search" button etc).
    const addButton = screen
      .getAllByRole('button')
      .find((el) => el.textContent?.includes('Add')) as HTMLElement;
    fireEvent.click(addButton);
    // Dialog appears with the title.
    await waitFor(() => expect(screen.getByText(/Add "Blade Runner"/)).toBeTruthy());

    fireEvent.click(screen.getByText('OK'));
    await waitFor(() =>
      expect(requestV3).toHaveBeenCalledWith(
        '/movie',
        expect.objectContaining({
          method: 'POST',
          body: expect.objectContaining({
            tmdbId: 78,
            rootFolderPath: '/movies',
            qualityProfileId: 'qp-1',
          }),
        })
      )
    );
    await waitFor(() => expect(screen.getByText('added')).toBeTruthy());
  });

  it('renders an error banner when both lookups fail', async () => {
    requestV3.mockRejectedValue(new Error('boom'));
    const { container } = renderPage();
    const input = container.querySelector('input[name="add-search"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'x' } });
    await waitFor(() => expect(screen.getByText(/Search failed/i)).toBeTruthy(), { timeout: 2000 });
  });
});
