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
    },
  };
});

vi.mock('next/navigation', () => ({
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
});
