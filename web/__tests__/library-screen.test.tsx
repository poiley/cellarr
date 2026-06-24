import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor, fireEvent } from '@testing-library/react';

// --- mocks ------------------------------------------------------------------
// The Library screen reads the singleton API client and Next's navigation
// hooks; we mock both so the component can be exercised in jsdom. Content is
// driven from the v3 catalogues (listMovies/listSeries), scoped to a library by
// its media type + root folders — not the sparse /api/v1 content refs.

const listLibraries = vi.fn();
const listMovies = vi.fn();
const listSeries = vi.fn();
const runCommandV3 = vi.fn();
const push = vi.fn();
let searchParams = new URLSearchParams();

vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      listLibraries: (...a: unknown[]) => listLibraries(...a),
      listMovies: (...a: unknown[]) => listMovies(...a),
      listSeries: (...a: unknown[]) => listSeries(...a),
      runCommandV3: (...a: unknown[]) => runCommandV3(...a),
    },
  };
});

vi.mock('next/navigation', () => ({
  usePathname: () => '/',
  useRouter: () => ({ push }),
  useSearchParams: () => searchParams,
}));

import LibraryPage from '@app/library/page';
import { ThemeProvider } from '@lib/ThemeProvider';

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
      <LibraryPage />
    </ThemeProvider>
  );
}

const LIBS = [
  { id: 'lib-movies', media_type: 'movie', name: 'Movies', root_folders: ['/movies'], default_quality_profile: 'p1' },
  { id: 'lib-tv', media_type: 'tv', name: 'TV', root_folders: ['/tv'], default_quality_profile: 'p2' },
];

const MOVIES = [
  {
    id: 'm1',
    title: 'Synthetic Movie One',
    year: 1999,
    monitored: true,
    hasFile: true,
    rootFolderPath: '/movies',
    sizeOnDisk: 8_000_000_000,
    movieFile: { quality: { quality: { name: 'Bluray-1080p' } } },
  },
  {
    id: 'm2',
    title: 'Synthetic Movie Two',
    year: 2005,
    monitored: false,
    hasFile: false,
    rootFolderPath: '/movies',
    sizeOnDisk: 0,
  },
];

const SERIES = [
  {
    id: 's1',
    title: 'Synthetic Series',
    monitored: true,
    hasFile: false,
    seriesType: 'standard',
    rootFolderPath: '/tv',
    sizeOnDisk: 0,
  },
];

describe('Library browse screen', () => {
  beforeEach(() => {
    installMatchMedia();
    window.localStorage.clear();
    searchParams = new URLSearchParams();
    listLibraries.mockReset();
    listMovies.mockReset();
    listSeries.mockReset();
    runCommandV3.mockReset();
    runCommandV3.mockResolvedValue({ id: 'cmd-1' });
    push.mockReset();
  });
  afterEach(() => cleanup());

  it('shows an empty state for a fresh install (no libraries)', async () => {
    listLibraries.mockResolvedValue([]);
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/No libraries yet/i)).toBeTruthy();
    });
  });

  it('lists libraries returned by the API', async () => {
    listLibraries.mockResolvedValue(LIBS);
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/Movies — movie/)).toBeTruthy();
      expect(screen.getByText(/TV — tv/)).toBeTruthy();
    });
  });

  it('auto-selects the first library and renders its items on load (no lib param)', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listMovies.mockResolvedValue(MOVIES);
    // No `lib` search param — the screen should still show content immediately
    // by defaulting to the first library, instead of only library names.
    renderPage();

    await waitFor(() => expect(listMovies).toHaveBeenCalled());
    await waitFor(() => {
      expect(screen.getByText('Synthetic Movie One')).toBeTruthy();
      expect(screen.getByText('Synthetic Movie Two')).toBeTruthy();
    });
    // The switcher remains usable: every library is still listed.
    expect(screen.getByText(/TV — tv/)).toBeTruthy();
  });

  it('falls back to the first library when the requested lib id is unknown', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listMovies.mockResolvedValue(MOVIES);
    searchParams = new URLSearchParams('lib=does-not-exist');
    renderPage();

    // Stale/bad id → still renders the first library's items rather than nothing.
    await waitFor(() => expect(listMovies).toHaveBeenCalled());
    await waitFor(() => {
      expect(screen.getByText('Synthetic Movie One')).toBeTruthy();
    });
  });

  it('renders the movies that belong to a movie library', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listMovies.mockResolvedValue(MOVIES);
    searchParams = new URLSearchParams('lib=lib-movies');
    renderPage();

    await waitFor(() => {
      expect(listMovies).toHaveBeenCalled();
    });
    await waitFor(() => {
      // Titles + year + quality + download/monitor state are all surfaced.
      expect(screen.getByText('Synthetic Movie One')).toBeTruthy();
      expect(screen.getByText('Synthetic Movie Two')).toBeTruthy();
      expect(screen.getByText('1999')).toBeTruthy();
      expect(screen.getByText('Bluray-1080p')).toBeTruthy();
      expect(screen.getAllByText('MONITORED').length).toBeGreaterThan(0);
      expect(screen.getAllByText('UNMONITORED').length).toBeGreaterThan(0);
      expect(screen.getAllByText('DOWNLOADED').length).toBeGreaterThan(0);
      expect(screen.getAllByText('MISSING').length).toBeGreaterThan(0);
    });
  });

  it('renders the series for a tv library', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listSeries.mockResolvedValue(SERIES);
    searchParams = new URLSearchParams('lib=lib-tv');
    renderPage();

    await waitFor(() => expect(listSeries).toHaveBeenCalled());
    await waitFor(() => {
      expect(screen.getByText('Synthetic Series')).toBeTruthy();
    });
  });

  it('drills into the item-detail screen when a row is clicked', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listMovies.mockResolvedValue(MOVIES);
    searchParams = new URLSearchParams('lib=lib-movies');
    renderPage();

    await waitFor(() => expect(screen.getByText('Synthetic Movie One')).toBeTruthy());
    fireEvent.click(screen.getByText('Synthetic Movie One'));

    await waitFor(() => {
      expect(push).toHaveBeenCalledWith('/content/?id=m1');
    });
  });

  it('filters content by the filter input', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listMovies.mockResolvedValue(MOVIES);
    searchParams = new URLSearchParams('lib=lib-movies');
    const { container } = renderPage();

    await waitFor(() => expect(screen.getByText('Synthetic Movie One')).toBeTruthy());

    const input = container.querySelector('input[name="content-filter"]') as HTMLInputElement;
    expect(input).toBeTruthy();
    fireEvent.change(input, { target: { value: 'Two' } });

    await waitFor(() => {
      expect(screen.queryByText('Synthetic Movie One')).toBeNull();
      expect(screen.getByText('Synthetic Movie Two')).toBeTruthy();
    });
  });

  it('shows an empty state when the selected library has no content', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listMovies.mockResolvedValue([]);
    searchParams = new URLSearchParams('lib=lib-movies');
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/This library is empty/i)).toBeTruthy();
    });
  });

  it('surfaces an error when content fails to load', async () => {
    const { ApiError } = await import('@lib/api/client');
    listLibraries.mockResolvedValue(LIBS);
    listMovies.mockRejectedValue(new ApiError('internal_error', 'boom', 500));
    searchParams = new URLSearchParams('lib=lib-movies');
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/Could not load content/i)).toBeTruthy();
    });
  });

  // --- #15 segmented control -------------------------------------------------

  it('renders the library switcher as a segmented control (tablist) and switches on click', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listMovies.mockResolvedValue(MOVIES);
    searchParams = new URLSearchParams('lib=lib-movies');
    renderPage();

    await waitFor(() => expect(screen.getByText('Synthetic Movie One')).toBeTruthy());

    // The switcher is a tablist with one tab per library, the active one marked.
    const tablist = screen.getByRole('tablist', { name: /libraries/i });
    expect(tablist).toBeTruthy();
    const tabs = screen.getAllByRole('tab');
    expect(tabs.length).toBe(2);
    const active = tabs.find((t) => t.getAttribute('aria-selected') === 'true');
    expect(active?.textContent).toMatch(/Movies/);

    // Clicking the TV segment deep-links via ?lib=.
    const tvTab = tabs.find((t) => /TV/.test(t.textContent ?? ''))!;
    fireEvent.click(tvTab);
    await waitFor(() => expect(push).toHaveBeenCalledWith('/library/?lib=lib-tv'));
  });

  // --- #16 sortable columns --------------------------------------------------

  it('sorts items by a column header click with aria-sort indicators', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listMovies.mockResolvedValue(MOVIES);
    searchParams = new URLSearchParams('lib=lib-movies');
    const { container } = renderPage();

    await waitFor(() => expect(screen.getByText('Synthetic Movie One')).toBeTruthy());

    const yearHeader = screen.getByRole('columnheader', { name: /Year/ });
    // Default sort is title-asc, so year header starts unsorted.
    expect(yearHeader.getAttribute('aria-sort')).toBe('none');

    fireEvent.click(yearHeader);
    await waitFor(() => expect(yearHeader.getAttribute('aria-sort')).toBe('ascending'));

    // Ascending year → 1999 row before 2005 row.
    const titles = Array.from(container.querySelectorAll('td[role="link"]')).map(
      (n) => n.textContent
    );
    expect(titles[0]).toBe('Synthetic Movie One'); // 1999
    expect(titles[1]).toBe('Synthetic Movie Two'); // 2005

    // A second click flips to descending.
    fireEvent.click(yearHeader);
    await waitFor(() => expect(yearHeader.getAttribute('aria-sort')).toBe('descending'));
    const titlesDesc = Array.from(container.querySelectorAll('td[role="link"]')).map(
      (n) => n.textContent
    );
    expect(titlesDesc[0]).toBe('Synthetic Movie Two');
  });

  // --- #17 status glyph ------------------------------------------------------

  it('shows a ✓/✗ status glyph so MISSING stands out beyond colour', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listMovies.mockResolvedValue(MOVIES);
    searchParams = new URLSearchParams('lib=lib-movies');
    renderPage();

    await waitFor(() => expect(screen.getByText('Synthetic Movie One')).toBeTruthy());
    // m1 is downloaded (✓), m2 is missing (✗).
    expect(screen.getByText('✓')).toBeTruthy();
    expect(screen.getByText('✗')).toBeTruthy();
  });

  // --- #18 type filter hidden for single-type libraries ----------------------

  it('hides the Type filter + column for a single-media-type library', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listMovies.mockResolvedValue(MOVIES);
    searchParams = new URLSearchParams('lib=lib-movies');
    const { container } = renderPage();

    await waitFor(() => expect(screen.getByText('Synthetic Movie One')).toBeTruthy());
    // No "Type" filter select, and no "Type" column header (all rows are movies).
    expect(container.querySelector('[name="content-type"]')).toBeNull();
    expect(screen.queryByRole('columnheader', { name: /^Type$/ })).toBeNull();
  });

  // --- #19 multi-select + bulk search ---------------------------------------

  it('multi-selects rows and bulk-searches the selection via the v3 command', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listMovies.mockResolvedValue(MOVIES);
    searchParams = new URLSearchParams('lib=lib-movies');
    const { container } = renderPage();

    await waitFor(() => expect(screen.getByText('Synthetic Movie One')).toBeTruthy());

    // Select the missing row (m2) by its row checkbox.
    const m2checkbox = container.querySelector('input[name="select-m2"]') as HTMLInputElement;
    expect(m2checkbox).toBeTruthy();
    fireEvent.click(m2checkbox);

    // Bulk bar appears; trigger the search-missing action.
    await waitFor(() => expect(screen.getByText(/1 selected/)).toBeTruthy());
    fireEvent.click(screen.getByText(/Search missing/));

    await waitFor(() =>
      expect(runCommandV3).toHaveBeenCalledWith({ name: 'MoviesSearch', movieId: 'm2' })
    );
  });
});
