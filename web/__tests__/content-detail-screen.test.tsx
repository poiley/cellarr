import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';

const getContent = vi.fn();
const listContentFiles = vi.fn();
const listContent = vi.fn();
const runCommand = vi.fn();
let searchParams = new URLSearchParams();

vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      getContent: (...a: unknown[]) => getContent(...a),
      listContentFiles: (...a: unknown[]) => listContentFiles(...a),
      listContent: (...a: unknown[]) => listContent(...a),
      runCommand: (...a: unknown[]) => runCommand(...a),
    },
  };
});

vi.mock('next/navigation', () => ({
  useRouter: () => ({ push: vi.fn() }),
  useSearchParams: () => searchParams,
}));

import ContentPage from '@app/content/page';
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
      <ContentPage />
    </ThemeProvider>
  );
}

const SERIES = {
  id: 'c-series',
  library_id: 'lib-tv',
  media_type: 'tv',
  kind: 'series',
  monitored: true,
  coords: { type: 'movie' },
  title: 'Breaking Bad',
};

const SIBLINGS = [
  { id: 'c-series', library_id: 'lib-tv', media_type: 'tv', kind: 'series', monitored: true, coords: { type: 'movie' }, title: 'Breaking Bad' },
  { id: 'c-s1', library_id: 'lib-tv', media_type: 'tv', kind: 'season', monitored: true, parent_id: 'c-series', coords: { type: 'seasonpack', season: 1 }, title: 'Season 1' },
  { id: 'c-e1', library_id: 'lib-tv', media_type: 'tv', kind: 'episode', monitored: true, parent_id: 'c-s1', coords: { type: 'episode', season: 1, episode: 1 }, title: 'Pilot' },
];

const FILES = [
  { id: 'f1', path: '/tv/Breaking Bad/S01E01.mkv', size: 1_500_000_000, quality: { name: 'Bluray-1080p', rank: 9 }, custom_format_score: 25 },
];

describe('Item-detail screen', () => {
  beforeEach(() => {
    installMatchMedia();
    window.localStorage.clear();
    searchParams = new URLSearchParams();
    getContent.mockReset();
    listContentFiles.mockReset();
    listContent.mockReset();
    runCommand.mockReset();
  });
  afterEach(() => cleanup());

  it('shows a no-selection state when no id is in the query', async () => {
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/No item selected/i)).toBeTruthy();
    });
  });

  it('renders the header, badges and files for a content node', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue(FILES);

    renderPage();

    await waitFor(() => {
      // Title appears in the CardDouble header.
      expect(screen.getAllByText('Breaking Bad').length).toBeGreaterThan(0);
      // Status + kind badges.
      expect(screen.getByText('series')).toBeTruthy();
      expect(screen.getByText('MONITORED')).toBeTruthy();
    });

    await waitFor(() => {
      // File table renders the basename + quality + formatted size.
      expect(screen.getByText('S01E01.mkv')).toBeTruthy();
      expect(screen.getAllByText('Bluray-1080p').length).toBeGreaterThan(0);
      expect(screen.getByText('1.4 GB')).toBeTruthy();
      expect(screen.getByText('+25')).toBeTruthy();
    });
  });

  it('renders the series→season→episode tree from sibling content', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue([]);

    renderPage();

    await waitFor(() => {
      // TreeView nodes render their titles (defaultValue expands them).
      expect(screen.getByText(/Season 1/)).toBeTruthy();
      expect(screen.getByText(/Pilot/)).toBeTruthy();
    });
  });

  it('shows an empty files state when nothing is imported', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue([]);

    renderPage();

    await waitFor(() => {
      expect(screen.getByText(/No files on disk yet/i)).toBeTruthy();
    });
  });

  it('surfaces an error when the item fails to load', async () => {
    const { ApiError } = await import('@lib/api/client');
    searchParams = new URLSearchParams('id=missing');
    getContent.mockRejectedValue(new ApiError('not_found', 'content missing not found', 404));
    listContentFiles.mockResolvedValue([]);

    renderPage();

    await waitFor(() => {
      expect(screen.getByText(/Could not load item/i)).toBeTruthy();
    });
  });
});
