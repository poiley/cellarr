import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

const getContent = vi.fn();
const listContentFiles = vi.fn();
const listContent = vi.fn();
const listMovies = vi.fn();
const listSeries = vi.fn();
const runCommand = vi.fn();
const getQualityProfiles = vi.fn();
const getLibrary = vi.fn();
const requestV3 = vi.fn();
const push = vi.fn();
let searchParams = new URLSearchParams();

vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      getContent: (...a: unknown[]) => getContent(...a),
      listContentFiles: (...a: unknown[]) => listContentFiles(...a),
      listContent: (...a: unknown[]) => listContent(...a),
      listMovies: (...a: unknown[]) => listMovies(...a),
      listSeries: (...a: unknown[]) => listSeries(...a),
      runCommand: (...a: unknown[]) => runCommand(...a),
      getQualityProfiles: (...a: unknown[]) => getQualityProfiles(...a),
      getLibrary: (...a: unknown[]) => getLibrary(...a),
      requestV3: (...a: unknown[]) => requestV3(...a),
    },
  };
});

vi.mock('next/navigation', () => ({
  usePathname: () => '/',
  useRouter: () => ({ push }),
  useSearchParams: () => searchParams,
}));

import ContentPage from '@app/content/page';
import { ThemeProvider } from '@lib/ThemeProvider';
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
      <ToastProvider>
        <ContentPage />
      </ToastProvider>
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
    listMovies.mockReset();
    listSeries.mockReset();
    runCommand.mockReset();
    getQualityProfiles.mockReset();
    getLibrary.mockReset();
    requestV3.mockReset();
    push.mockReset();
    // Default: empty catalogues unless a test overrides them.
    listMovies.mockResolvedValue([]);
    listSeries.mockResolvedValue([]);
    getQualityProfiles.mockResolvedValue([]);
    getLibrary.mockResolvedValue({ id: 'lib-tv', name: 'TV Shows' });
    // The v3 detail resource (GET) and monitored-PUT both go through requestV3;
    // default to a minimal detail resource that echoes the requested body.
    requestV3.mockImplementation((_path: string, opts?: { body?: { monitored?: boolean } }) =>
      Promise.resolve({
        id: 'c-series',
        title: 'Breaking Bad',
        monitored: opts?.body?.monitored ?? true,
        qualityProfileId: 'qp-1',
        sizeOnDisk: 1_500_000_000,
        hasFile: true,
        status: 'continuing',
        overview: '',
        year: 0,
      })
    );
    // runCommand resolves like the real client (CommandAccepted) so the screen's
    // `.then(res => res.status)` success path does not reject.
    runCommand.mockResolvedValue({ job_id: 'job-1', name: 'RefreshSeries', status: 'queued' });
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
      // Kind badge.
      expect(screen.getByText('series')).toBeTruthy();
    });
    // Status badge now prefers the v3 detail's status (#20).
    await waitFor(() => {
      expect(screen.getByText('CONTINUING')).toBeTruthy();
      // The Monitored toggle reflects the current state (#21).
      expect(screen.getByRole('button', { name: /Monitored/ })).toBeTruthy();
    });

    await waitFor(() => {
      // File table renders the basename + quality + formatted size.
      expect(screen.getByText('S01E01.mkv')).toBeTruthy();
      expect(screen.getAllByText('Bluray-1080p').length).toBeGreaterThan(0);
      // '1.4 GB' now appears in both the metadata block and the file row.
      expect(screen.getAllByText('1.4 GB').length).toBeGreaterThan(0);
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

  it('resolves the title from the v3 catalogue when the node carries none', async () => {
    // The real /api/v1/content/{id} node has no title — only a title_id.
    const NODE = { id: 'cdb67951', library_id: 'lib-movies', media_type: 'movie', kind: 'movie', monitored: true, coords: { type: 'movie' }, title_id: 'tid-1' };
    searchParams = new URLSearchParams('id=cdb67951');
    getContent.mockResolvedValue(NODE);
    listContent.mockResolvedValue([]);
    listContentFiles.mockResolvedValue([]);
    listMovies.mockResolvedValue([{ id: 'cdb67951', title: 'Synthetic Movie Two' }]);
    listSeries.mockResolvedValue([]);

    renderPage();

    await waitFor(() => {
      expect(screen.getAllByText('Synthetic Movie Two').length).toBeGreaterThan(0);
      // The raw #shortid fallback must NOT be shown.
      expect(screen.queryByText('#cdb6795')).toBeNull();
    });
  });

  it('navigates to interactive search and history with the content id', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue([]);

    renderPage();

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /Search/ })).toBeTruthy();
      expect(screen.getByRole('button', { name: /^⌘H\s*History$/ })).toBeTruthy();
    });

    fireEvent.click(screen.getByRole('button', { name: /Search/ }));
    expect(push).toHaveBeenCalledWith(expect.stringContaining('/interactive?id=c-series'));
    expect(push).toHaveBeenCalledWith(expect.stringContaining('content=c-series'));

    push.mockClear();
    fireEvent.click(screen.getByRole('button', { name: /^⌘H\s*History$/ }));
    expect(push).toHaveBeenCalledWith('/history?id=c-series');

    // Refresh on a TV node must send the backend-accepted RefreshSeries command
    // (NOT the rejected RefreshContent). See commands.rs `kind_for_command`.
    fireEvent.click(screen.getByRole('button', { name: /Refresh/ }));
    expect(runCommand).toHaveBeenCalledWith('RefreshSeries', 'c-series');
    expect(runCommand).not.toHaveBeenCalledWith('RefreshContent', expect.anything());
  });

  it('renders the metadata block from the v3 detail resource (#20)', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue([]);
    getQualityProfiles.mockResolvedValue([{ id: 'qp-1', name: 'HD-1080p' }]);

    renderPage();

    await waitFor(() => {
      // Quality-profile id is resolved to its display name.
      expect(screen.getByText('HD-1080p')).toBeTruthy();
      // Total size + status labels render.
      expect(screen.getByText('Total size')).toBeTruthy();
      expect(screen.getByText('1.4 GB')).toBeTruthy();
      expect(screen.getByText('Quality profile')).toBeTruthy();
    });
  });

  it('uses the library name as the middle breadcrumb crumb (#24)', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue([]);
    getLibrary.mockResolvedValue({ id: 'lib-tv', name: 'TV Shows' });

    renderPage();

    await waitFor(() => {
      expect(screen.getByText('TV Shows')).toBeTruthy();
      // The generic 'Content' crumb must NOT linger once the name resolves.
      expect(screen.queryByText('Content')).toBeNull();
    });
  });

  it('toggles monitored via PUT and shows toast feedback (#21)', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue([]);

    renderPage();

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /Monitored/ })).toBeTruthy();
    });

    requestV3.mockClear();
    fireEvent.click(screen.getByRole('button', { name: /Monitored/ }));

    await waitFor(() => {
      // A PUT to the series detail resource flipping monitored -> false.
      expect(requestV3).toHaveBeenCalledWith(
        '/series/c-series',
        expect.objectContaining({ method: 'PUT', body: { monitored: false } })
      );
      // Toast confirms the result.
      expect(screen.getByText(/Monitoring disabled/i)).toBeTruthy();
    });
  });

  it('hides the Score column when no file carries a score (#23)', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue([
      { id: 'f1', path: '/tv/x/S01E01.mkv', size: 1000, quality: { name: 'WEBDL-1080p' } },
    ]);

    renderPage();

    await waitFor(() => {
      expect(screen.getByText('S01E01.mkv')).toBeTruthy();
    });
    // No scores anywhere -> the Score header column is omitted.
    expect(screen.queryByText('Score')).toBeNull();
  });

  it('keeps the Score column when at least one file is scored (#23)', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue(FILES);

    renderPage();

    await waitFor(() => {
      expect(screen.getByText('Score')).toBeTruthy();
      expect(screen.getByText('+25')).toBeTruthy();
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
