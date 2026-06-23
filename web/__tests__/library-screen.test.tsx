import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor, within, fireEvent } from '@testing-library/react';

// --- mocks ------------------------------------------------------------------
// The Library screen reads the singleton API client and Next's navigation
// hooks; we mock both so the component can be exercised in jsdom.

const listLibraries = vi.fn();
const listContent = vi.fn();
const push = vi.fn();
let searchParams = new URLSearchParams();

vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      listLibraries: (...a: unknown[]) => listLibraries(...a),
      listContent: (...a: unknown[]) => listContent(...a),
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

const CONTENT = [
  { id: 'c1', library_id: 'lib-tv', media_type: 'tv', kind: 'series', monitored: true, coords: { type: 'movie' }, title: 'Breaking Bad' },
  { id: 'c2', library_id: 'lib-tv', media_type: 'tv', kind: 'episode', monitored: false, coords: { type: 'episode', season: 2, episode: 5 }, title: 'Episode Five' },
];

describe('Library browse screen', () => {
  beforeEach(() => {
    installMatchMedia();
    window.localStorage.clear();
    searchParams = new URLSearchParams();
    listLibraries.mockReset();
    listContent.mockReset();
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

  it('loads and renders the selected library content in a table', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listContent.mockResolvedValue(CONTENT);
    searchParams = new URLSearchParams('lib=lib-tv');
    renderPage();

    await waitFor(() => {
      expect(listContent).toHaveBeenCalledWith('lib-tv', expect.anything());
    });
    await waitFor(() => {
      expect(screen.getByText('Breaking Bad')).toBeTruthy();
      // status badge text from monitoredLabel
      expect(screen.getAllByText('MONITORED').length).toBeGreaterThan(0);
      expect(screen.getAllByText('UNMONITORED').length).toBeGreaterThan(0);
    });
  });

  it('filters content by the filter input', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listContent.mockResolvedValue(CONTENT);
    searchParams = new URLSearchParams('lib=lib-tv');
    const { container } = renderPage();

    await waitFor(() => expect(screen.getByText('Breaking Bad')).toBeTruthy());

    const input = container.querySelector('input[name="content-filter"]') as HTMLInputElement;
    expect(input).toBeTruthy();
    fireEvent.change(input, { target: { value: 'Five' } });

    await waitFor(() => {
      expect(screen.queryByText('Breaking Bad')).toBeNull();
      expect(screen.getByText('Episode Five')).toBeTruthy();
    });
  });

  it('shows an empty state when the selected library has no content', async () => {
    listLibraries.mockResolvedValue(LIBS);
    listContent.mockResolvedValue([]);
    searchParams = new URLSearchParams('lib=lib-movies');
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/This library is empty/i)).toBeTruthy();
    });
  });

  it('surfaces an error when content fails to load', async () => {
    const { ApiError } = await import('@lib/api/client');
    listLibraries.mockResolvedValue(LIBS);
    listContent.mockRejectedValue(new ApiError('internal_error', 'boom', 500));
    searchParams = new URLSearchParams('lib=lib-tv');
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/Could not load content/i)).toBeTruthy();
    });
  });
});
