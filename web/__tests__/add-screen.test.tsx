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
const getQualityProfiles = vi.fn();
vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      requestV3: (...args: unknown[]) => requestV3(...args),
      listLibraries: (...args: unknown[]) => listLibraries(...args),
      listRootFolders: (...args: unknown[]) => listRootFolders(...args),
      getQualityProfiles: (...args: unknown[]) => getQualityProfiles(...args),
    },
  };
});

import AddPage from '@app/add/page';
import { ToastProvider } from '@app/_lib/ToastProvider';

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
        <ToastProvider>
          <ModalProvider>
            <AddPage />
          </ModalProvider>
        </ToastProvider>
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
    getQualityProfiles.mockResolvedValue([{ id: 'qp-1', name: 'HD-1080p' }]);
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

  it('splits results into MOVIES and TV sections', async () => {
    const series = {
      title: 'Blade Runner: Black Lotus',
      titleSlug: 'blade-runner-black-lotus',
      year: 2021,
      tvdbId: 999,
      overview: 'An anime series.',
      monitored: false,
      hasFile: false,
      status: 'continuing',
    };
    requestV3.mockImplementation((path: string) => {
      if (path === '/movie/lookup') return Promise.resolve([bladeRunner]);
      if (path === '/series/lookup') return Promise.resolve([series]);
      return Promise.resolve(undefined);
    });
    const { container } = renderPage();
    const input = container.querySelector('input[name="add-search"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'blade' } });

    await waitFor(() => expect(screen.getByText('MOVIES')).toBeTruthy(), { timeout: 2000 });
    expect(screen.getByText('TV')).toBeTruthy();
    expect(screen.getByText('Blade Runner')).toBeTruthy();
    expect(screen.getByText('Blade Runner: Black Lotus')).toBeTruthy();
  });

  it('ranks the exact-title hit first within a section', async () => {
    const popular = { ...bladeRunner, title: 'Blade Runner 2049', tmdbId: 335984, popularity: 90 };
    const exact = { ...bladeRunner, title: 'Blade', tmdbId: 36647, popularity: 10 };
    requestV3.mockImplementation((path: string) => {
      if (path === '/movie/lookup') return Promise.resolve([popular, exact]);
      if (path === '/series/lookup') return Promise.resolve([]);
      return Promise.resolve(undefined);
    });
    const { container } = renderPage();
    const input = container.querySelector('input[name="add-search"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'blade' } });

    await waitFor(() => expect(screen.getByText('Blade')).toBeTruthy(), { timeout: 2000 });
    const cells = Array.from(container.querySelectorAll('td, th')).map((c) => c.textContent ?? '');
    expect(cells.indexOf('Blade')).toBeLessThan(cells.indexOf('Blade Runner 2049'));
    // Disambiguation aid surfaces popularity.
    expect(screen.getByText(/pop 90/)).toBeTruthy();
  });

  it('lets the dialog choose monitor/search and threads them into the POST', async () => {
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

    const addButton = screen
      .getAllByRole('button')
      .find((el) => el.textContent?.includes('Add')) as HTMLElement;
    fireEvent.click(addButton);
    await waitFor(() => expect(screen.getByText(/Add "Blade Runner"/)).toBeTruthy());

    // The dialog exposes the field selects + checkboxes.
    expect(container.querySelector('input[name="add-monitor"]')).toBeTruthy();
    const searchBox = container.querySelector(
      'input[name="add-search-on-add"]'
    ) as HTMLInputElement;
    expect(searchBox).toBeTruthy();
    // Untick "search on add".
    fireEvent.click(searchBox);

    fireEvent.click(screen.getByText('OK'));
    await waitFor(() =>
      expect(requestV3).toHaveBeenCalledWith(
        '/movie',
        expect.objectContaining({
          method: 'POST',
          body: expect.objectContaining({
            monitored: true,
            addOptions: { searchForMovie: false },
          }),
        })
      )
    );
  });

  it('shows a success toast with a View link after add', async () => {
    requestV3.mockImplementation((path: string) => {
      if (path === '/movie/lookup') return Promise.resolve([bladeRunner]);
      if (path === '/series/lookup') return Promise.resolve([]);
      if (path === '/movie') return Promise.resolve({ id: 'c-new' });
      return Promise.resolve(undefined);
    });
    const { container } = renderPage();
    const input = container.querySelector('input[name="add-search"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'blade' } });
    await waitFor(() => expect(screen.getByText('Blade Runner')).toBeTruthy(), { timeout: 2000 });

    const addButton = screen
      .getAllByRole('button')
      .find((el) => el.textContent?.includes('Add')) as HTMLElement;
    fireEvent.click(addButton);
    await waitFor(() => expect(screen.getByText(/Add "Blade Runner"/)).toBeTruthy());
    fireEvent.click(screen.getByText('OK'));

    await waitFor(() => {
      const view = screen.getByText(/View/) as HTMLAnchorElement;
      expect(view.getAttribute('href')).toBe('/content?id=c-new');
    });
  });

  it('offers a monitor-options dropdown for a series and threads it into the POST', async () => {
    const series = {
      title: 'Breaking Bad',
      titleSlug: 'breaking-bad',
      year: 2008,
      tvdbId: 81189,
      overview: 'A chemistry teacher.',
      monitored: false,
      hasFile: false,
      status: 'ended',
    };
    requestV3.mockImplementation((path: string) => {
      if (path === '/movie/lookup') return Promise.resolve([]);
      if (path === '/series/lookup') return Promise.resolve([series]);
      if (path === '/series') return Promise.resolve({ id: 'c-series' });
      return Promise.resolve(undefined);
    });
    const { container } = renderPage();
    const input = container.querySelector('input[name="add-search"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'breaking' } });
    await waitFor(() => expect(screen.getByText('Breaking Bad')).toBeTruthy(), { timeout: 2000 });

    const addButton = screen
      .getAllByRole('button')
      .find((el) => el.textContent?.includes('Add')) as HTMLElement;
    fireEvent.click(addButton);
    await waitFor(() => expect(screen.getByText(/Add "Breaking Bad"/)).toBeTruthy());

    // The series dialog exposes the monitor-options Select (not the bare checkbox).
    expect(container.querySelector('input[name="add-monitor"]')).toBeNull();
    // The dialog has several Selects (profile / root / monitor); the monitor
    // dropdown is the last one rendered.
    const selects = container.querySelectorAll('button[aria-haspopup="listbox"]');
    const select = selects[selects.length - 1] as HTMLElement;
    expect(select).toBeTruthy();
    // The default option is "All episodes".
    expect(screen.getByText('All episodes')).toBeTruthy();

    // Choose "First season".
    fireEvent.click(select);
    fireEvent.click(screen.getByText('First season'));

    fireEvent.click(screen.getByText('OK'));
    await waitFor(() =>
      expect(requestV3).toHaveBeenCalledWith(
        '/series',
        expect.objectContaining({
          method: 'POST',
          body: expect.objectContaining({
            tvdbId: 81189,
            monitored: true,
            addOptions: expect.objectContaining({ monitor: 'firstSeason' }),
          }),
        })
      )
    );
  });

  it('adds a series unmonitored when the monitor option is None', async () => {
    const series = {
      title: 'Breaking Bad',
      titleSlug: 'breaking-bad',
      year: 2008,
      tvdbId: 81189,
      monitored: false,
      hasFile: false,
      status: 'ended',
    };
    requestV3.mockImplementation((path: string) => {
      if (path === '/movie/lookup') return Promise.resolve([]);
      if (path === '/series/lookup') return Promise.resolve([series]);
      if (path === '/series') return Promise.resolve({ id: 'c-series' });
      return Promise.resolve(undefined);
    });
    const { container } = renderPage();
    const input = container.querySelector('input[name="add-search"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'breaking' } });
    await waitFor(() => expect(screen.getByText('Breaking Bad')).toBeTruthy(), { timeout: 2000 });

    const addButton = screen
      .getAllByRole('button')
      .find((el) => el.textContent?.includes('Add')) as HTMLElement;
    fireEvent.click(addButton);
    await waitFor(() => expect(screen.getByText(/Add "Breaking Bad"/)).toBeTruthy());

    // The dialog has several Selects (profile / root / monitor); the monitor
    // dropdown is the last one rendered.
    const selects = container.querySelectorAll('button[aria-haspopup="listbox"]');
    const select = selects[selects.length - 1] as HTMLElement;
    fireEvent.click(select);
    fireEvent.click(screen.getByText('None (unmonitored)'));

    fireEvent.click(screen.getByText('OK'));
    await waitFor(() =>
      expect(requestV3).toHaveBeenCalledWith(
        '/series',
        expect.objectContaining({
          method: 'POST',
          body: expect.objectContaining({
            monitored: false,
            addOptions: expect.objectContaining({ monitor: 'none' }),
          }),
        })
      )
    );
  });

  it('offers a series-type selector and posts the chosen seriesType (anime)', async () => {
    const series = {
      title: 'Frieren',
      titleSlug: 'frieren',
      year: 2023,
      tvdbId: 424536,
      overview: 'An elf mage.',
      monitored: false,
      hasFile: false,
      status: 'continuing',
    };
    requestV3.mockImplementation((path: string) => {
      if (path === '/movie/lookup') return Promise.resolve([]);
      if (path === '/series/lookup') return Promise.resolve([series]);
      if (path === '/series') return Promise.resolve({ id: 'c-series' });
      return Promise.resolve(undefined);
    });
    const { container } = renderPage();
    const input = container.querySelector('input[name="add-search"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'frieren' } });
    await waitFor(() => expect(screen.getByText('Frieren')).toBeTruthy(), { timeout: 2000 });

    const addButton = screen
      .getAllByRole('button')
      .find((el) => el.textContent?.includes('Add')) as HTMLElement;
    fireEvent.click(addButton);
    await waitFor(() => expect(screen.getByText(/Add "Frieren"/)).toBeTruthy());

    // The series dialog exposes a Series Type select with an accessible name.
    const typeSelect = screen.getByRole('button', { name: 'Series type' });
    expect(typeSelect).toBeTruthy();
    // The default is "Standard".
    expect(screen.getByText('Standard')).toBeTruthy();

    // Choose "Anime".
    fireEvent.click(typeSelect);
    fireEvent.click(screen.getByText('Anime'));

    fireEvent.click(screen.getByText('OK'));
    await waitFor(() =>
      expect(requestV3).toHaveBeenCalledWith(
        '/series',
        expect.objectContaining({
          method: 'POST',
          body: expect.objectContaining({
            tvdbId: 424536,
            seriesType: 'anime',
          }),
        })
      )
    );
  });

  it('does not send seriesType on a movie add', async () => {
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

    const addButton = screen
      .getAllByRole('button')
      .find((el) => el.textContent?.includes('Add')) as HTMLElement;
    fireEvent.click(addButton);
    await waitFor(() => expect(screen.getByText(/Add "Blade Runner"/)).toBeTruthy());
    // A movie dialog has no series-type select.
    expect(screen.queryByRole('button', { name: 'Series type' })).toBeNull();

    fireEvent.click(screen.getByText('OK'));
    await waitFor(() => {
      const post = requestV3.mock.calls.find(
        ([p, o]) => p === '/movie' && (o as { method?: string })?.method === 'POST'
      );
      expect(post).toBeTruthy();
      const body = (post![1] as { body?: Record<string, unknown> }).body ?? {};
      expect(body).not.toHaveProperty('seriesType');
    });
  });

  it('renders an error banner when both lookups fail', async () => {
    requestV3.mockRejectedValue(new Error('boom'));
    const { container } = renderPage();
    const input = container.querySelector('input[name="add-search"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'x' } });
    await waitFor(() => expect(screen.getByText(/Search failed/i)).toBeTruthy(), { timeout: 2000 });
  });
});
