import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

const getContent = vi.fn();
const listContentFiles = vi.fn();
const listContent = vi.fn();
const listMovies = vi.fn();
const listSeries = vi.fn();
const listEpisodes = vi.fn();
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
      listEpisodes: (...a: unknown[]) => listEpisodes(...a),
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

// The v3 episode rows the monitor tree is sourced from (GET /api/v3/episode).
// Ids come back as the numeric projection, the same value the monitor routes
// accept. The season node id (`c-s1`) is resolved from SIBLINGS by season number.
const EPISODES = [
  { id: 1001, seriesId: 42, seasonNumber: 1, episodeNumber: 1, title: 'Pilot', monitored: true, hasFile: true, airDate: '2008-01-20' },
  { id: 1002, seriesId: 42, seasonNumber: 1, episodeNumber: 2, title: 'Cat in the Bag', monitored: true, hasFile: false, airDate: '2008-01-27' },
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
    listEpisodes.mockReset();
    runCommand.mockReset();
    getQualityProfiles.mockReset();
    getLibrary.mockReset();
    requestV3.mockReset();
    push.mockReset();
    // Default: empty catalogues unless a test overrides them.
    listMovies.mockResolvedValue([]);
    listSeries.mockResolvedValue([]);
    // Default: no episodes (so the Monitoring card is absent unless a test seeds
    // the v3 episode endpoint). The per-season/episode monitor tree is sourced
    // from GET /api/v3/episode?seriesId=… (the real `list_episodes`).
    listEpisodes.mockResolvedValue([]);
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
    // Status badge now prefers the v3 detail's status (#20). The header badge and
    // the metadata "Status" row use the SAME uppercased label (casing alignment),
    // so it appears in both places.
    await waitFor(() => {
      expect(screen.getAllByText('CONTINUING').length).toBeGreaterThan(0);
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
      // TreeView nodes render their titles (defaultValue expands them). The
      // titles also appear in the Monitoring card below, so allow multiple.
      expect(screen.getAllByText(/Season 1/).length).toBeGreaterThan(0);
      expect(screen.getAllByText(/Pilot/).length).toBeGreaterThan(0);
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

  it('renders the poster from the mediacover endpoint and shows year + runtime (#20)', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue([]);
    // A detail resource now carrying real year/overview/runtime.
    requestV3.mockResolvedValue({
      id: 'c-series',
      title: 'Breaking Bad',
      monitored: true,
      qualityProfileId: 'qp-1',
      sizeOnDisk: 1_500_000_000,
      hasFile: true,
      status: 'continuing',
      overview: 'A high-school chemistry teacher turned methamphetamine producer.',
      year: 2008,
      runtime: 47,
    });

    renderPage();

    await waitFor(() => {
      // The poster <img> points at the cached-artwork endpoint for this id.
      const img = screen.getByAltText('Breaking Bad poster') as HTMLImageElement;
      expect(img.getAttribute('src')).toContain('/api/v3/mediacover/c-series/poster');
    });
    // Year + runtime + overview surface in the metadata block.
    expect(screen.getByText('Year')).toBeTruthy();
    expect(screen.getByText('2008')).toBeTruthy();
    expect(screen.getByText('Runtime')).toBeTruthy();
    expect(screen.getByText('47m')).toBeTruthy();
    expect(
      screen.getByText(/high-school chemistry teacher turned methamphetamine producer/i)
    ).toBeTruthy();
  });

  it('falls back to an ASCII placeholder when the poster 404s', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue([]);

    renderPage();

    let img: HTMLImageElement | null = null;
    await waitFor(() => {
      img = screen.getByAltText('Breaking Bad poster') as HTMLImageElement;
      expect(img).toBeTruthy();
    });
    // Simulate the endpoint 404 — the screen swaps in the placeholder card.
    fireEvent.error(img!);
    await waitFor(() => {
      expect(screen.getByText(/No poster/i)).toBeTruthy();
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

  it('renders a per-season/episode Monitoring tree sourced from GET /api/v3/episode', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue([]);
    listEpisodes.mockResolvedValue(EPISODES);

    renderPage();

    await waitFor(() => {
      // The tree is fetched from the real `list_episodes` for this series id.
      expect(listEpisodes).toHaveBeenCalledWith('c-series', expect.anything());
      // The Monitoring card surfaces the season + its episodes with toggle buttons.
      expect(screen.getByText('Monitoring')).toBeTruthy();
      expect(screen.getByRole('button', { name: /Unmonitor Season 1/i })).toBeTruthy();
      // Each episode row carries its SxxEyy coordinate, title and on-disk badge.
      expect(screen.getByText(/S01E01 Pilot/)).toBeTruthy();
      expect(screen.getByText(/S01E02 Cat in the Bag/)).toBeTruthy();
      // hasFile state surfaces as ✓ file / ✗ missing badges (the legend line
      // also mentions both glyphs, so allow it among the matches).
      expect(screen.getAllByText(/✓ file/).length).toBeGreaterThan(0);
      expect(screen.getAllByText(/✗ missing/).length).toBeGreaterThan(0);
    });
  });

  it('toggles a season via the season-monitor route using the resolved season id', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue([]);
    listEpisodes.mockResolvedValue(EPISODES);
    requestV3.mockImplementation((path: string, opts?: { body?: { monitored?: boolean } }) => {
      if (path === '/season/monitor') {
        return Promise.resolve({
          seasonId: 'c-s1',
          monitored: opts?.body?.monitored ?? false,
          episodesUpdated: 2,
        });
      }
      return Promise.resolve({
        id: 'c-series',
        title: 'Breaking Bad',
        monitored: opts?.body?.monitored ?? true,
        status: 'continuing',
      });
    });

    renderPage();

    // Both episodes monitored -> the season reads as monitored; toggling it OFF
    // PUTs the season-monitor route with the season node id resolved from siblings.
    const seasonBtn = await screen.findByRole('button', { name: /Unmonitor Season 1/i });
    requestV3.mockClear();
    fireEvent.click(seasonBtn);

    await waitFor(() =>
      expect(requestV3).toHaveBeenCalledWith(
        '/season/monitor',
        expect.objectContaining({
          method: 'PUT',
          body: { seasonId: 'c-s1', monitored: false },
        })
      )
    );
    // The cascade flips both episode overrides off and surfaces the count.
    await waitFor(() => {
      expect(screen.getByText(/Stopped monitoring Season 1/i)).toBeTruthy();
      expect(screen.getByRole('button', { name: /Monitor S01E01 Pilot/i })).toBeTruthy();
    });
  });

  it('resolves the season node id from the kind-less v1 content projection (coords shape)', async () => {
    // The live `/api/v1/libraries/{id}/content` projection carries only
    // id/media_type/coords — NO `kind` / `parent_id`. The season node must still
    // be identified (season N with episode 0) so the season toggle uses the
    // `/season/monitor` route rather than falling back to a bulk episode cascade.
    const COORDS_ONLY_SIBLINGS = [
      { id: 'c-series', media_type: 'tv', coords: { type: 'episode', season: 0, episode: 0 } },
      { id: 'c-s1', media_type: 'tv', coords: { type: 'episode', season: 1, episode: 0 } },
      { id: 'c-e1', media_type: 'tv', coords: { type: 'episode', season: 1, episode: 1 } },
      { id: 'c-e2', media_type: 'tv', coords: { type: 'episode', season: 1, episode: 2 } },
    ];
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(COORDS_ONLY_SIBLINGS);
    listContentFiles.mockResolvedValue([]);
    listEpisodes.mockResolvedValue(EPISODES);
    requestV3.mockImplementation((path: string, opts?: { body?: { monitored?: boolean } }) => {
      if (path === '/season/monitor') {
        return Promise.resolve({ seasonId: 'c-s1', monitored: opts?.body?.monitored ?? false, episodesUpdated: 2 });
      }
      return Promise.resolve({ id: 'c-series', title: 'Breaking Bad', monitored: true, status: 'continuing' });
    });

    renderPage();

    const seasonBtn = await screen.findByRole('button', { name: /Unmonitor Season 1/i });
    requestV3.mockClear();
    fireEvent.click(seasonBtn);

    await waitFor(() =>
      // The season id resolved from coords -> the season-monitor route is used.
      expect(requestV3).toHaveBeenCalledWith(
        '/season/monitor',
        expect.objectContaining({ method: 'PUT', body: { seasonId: 'c-s1', monitored: false } })
      )
    );
  });

  it('toggles a single episode via the episode-monitor route with its numeric id', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue([]);
    listEpisodes.mockResolvedValue(EPISODES);
    requestV3.mockImplementation((path: string, opts?: { body?: { monitored?: boolean } }) => {
      if (path === '/episode/monitor') {
        return Promise.resolve({ updated: 1, monitored: opts?.body?.monitored ?? false });
      }
      return Promise.resolve({
        id: 'c-series',
        title: 'Breaking Bad',
        monitored: opts?.body?.monitored ?? true,
        status: 'continuing',
      });
    });

    renderPage();

    // The Pilot episode is monitored -> its row toggle reads "Unmonitor"; clicking
    // it PUTs the episode-monitor route with the episode's numeric projection id.
    const epBtn = await screen.findByRole('button', { name: /Unmonitor S01E01 Pilot/i });
    requestV3.mockClear();
    fireEvent.click(epBtn);

    await waitFor(() =>
      expect(requestV3).toHaveBeenCalledWith(
        '/episode/monitor',
        expect.objectContaining({
          method: 'PUT',
          body: { episodeIds: ['1001'], monitored: false },
        })
      )
    );
    await waitFor(() =>
      expect(screen.getByText(/Stopped monitoring Pilot/i)).toBeTruthy()
    );
  });

  it('shows no Monitoring card for a movie (no episode endpoint hit)', async () => {
    const MOVIE = { id: 'c-movie', library_id: 'lib-movies', media_type: 'movie', kind: 'movie', monitored: true, coords: { type: 'movie' }, title: 'Blade Runner' };
    searchParams = new URLSearchParams('id=c-movie');
    getContent.mockResolvedValue(MOVIE);
    listContent.mockResolvedValue([MOVIE]);
    listContentFiles.mockResolvedValue([]);
    getLibrary.mockResolvedValue({ id: 'lib-movies', name: 'Movies' });

    renderPage();

    await waitFor(() => {
      expect(screen.getAllByText('Blade Runner').length).toBeGreaterThan(0);
    });
    expect(screen.queryByText('Monitoring')).toBeNull();
    // A movie node never queries the per-series episode endpoint.
    expect(listEpisodes).not.toHaveBeenCalled();
  });

  it('shows no Monitoring card for a series with no episodes', async () => {
    searchParams = new URLSearchParams('id=c-series');
    getContent.mockResolvedValue(SERIES);
    listContent.mockResolvedValue(SIBLINGS);
    listContentFiles.mockResolvedValue([]);
    listEpisodes.mockResolvedValue([]);

    renderPage();

    await waitFor(() => {
      expect(screen.getAllByText('Breaking Bad').length).toBeGreaterThan(0);
    });
    // The endpoint was queried, but an empty result renders no Monitoring card.
    expect(listEpisodes).toHaveBeenCalledWith('c-series', expect.anything());
    expect(screen.queryByText('Monitoring')).toBeNull();
  });
});
