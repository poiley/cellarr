import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

const getHistory = vi.fn();
const getHistoryV3 = vi.fn();

vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      getHistory: (...a: unknown[]) => getHistory(...a),
      getHistoryV3: (...a: unknown[]) => getHistoryV3(...a),
    },
  };
});

// The History screen is URL-driven (`?id=` selects a node timeline; otherwise the
// global recent feed). Drive the active node through a mutable search-params stub.
let searchParams = new URLSearchParams();
vi.mock('next/navigation', () => ({
  usePathname: () => '/',
  useSearchParams: () => searchParams,
  useRouter: () => ({ push: () => {} }),
}));

import HistoryPage from '@app/history/page';
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
      <HistoryPage />
    </ThemeProvider>
  );
}

// Per-node timeline events (native `/api/v1/history?content=…`).
const EVENTS = [
  { at: '2024-01-01T10:00:00Z', content_id: 'c-1', run_id: 'run-xyz', event: { event: 'grabbed', grab_id: 'g-1' } },
  { at: '2024-01-01T11:00:00Z', content_id: 'c-1', run_id: 'run-xyz', event: { event: 'imported', grab_id: 'g-1' } },
  { at: '2024-01-01T12:00:00Z', content_id: 'c-1', run_id: 'run-xyz', event: { event: 'download_failed', grab_id: 'g-1', detail: 'tracker timeout' } },
];

// The global recent feed (`/api/v3/history` → Page<HistoryRecordV3>).
const FEED = {
  page: 1,
  pageSize: 3,
  totalRecords: 3,
  sortKey: 'date',
  sortDirection: 'descending',
  records: [
    { date: 1704103200, eventType: 'grabbed', sourceTitle: 'The Matrix', contentId: 'c-1', runId: 'run-xyz' },
    { date: 1704106800, eventType: 'downloadFolderImported', sourceTitle: 'The Matrix', contentId: 'c-1', runId: 'run-xyz' },
    { date: 1704110400, eventType: 'downloadFailed', sourceTitle: 'The Matrix', contentId: 'c-1', runId: 'run-abc' },
  ],
};

describe('History screen', () => {
  beforeEach(() => {
    installMatchMedia();
    window.localStorage.clear();
    searchParams = new URLSearchParams();
    getHistory.mockReset();
    getHistoryV3.mockReset();
  });
  afterEach(() => cleanup());

  it('loads the global recent feed by default (no node id required)', async () => {
    getHistoryV3.mockResolvedValue(FEED);
    renderPage();
    await waitFor(() => {
      expect(getHistoryV3).toHaveBeenCalled();
    });
    await waitFor(() => {
      // Both V3 event labels are humanized via v3EventLabel.
      expect(screen.getByText('Grabbed')).toBeTruthy();
      expect(screen.getByText('Imported')).toBeTruthy();
      expect(screen.getByText('Download failed')).toBeTruthy();
    });
    // The default feed does not fetch any single node's timeline.
    expect(getHistory).not.toHaveBeenCalled();
  });

  it('links each global-feed event to the decision log for its run', async () => {
    getHistoryV3.mockResolvedValue(FEED);
    const { container } = renderPage();
    await waitFor(() => {
      const links = container.querySelectorAll('a[href^="/decision-log?run="]');
      expect(links.length).toBe(FEED.records.length);
      expect((links[0] as HTMLAnchorElement).getAttribute('href')).toContain('run=run-xyz');
    });
  });

  it('shows the empty state when there is no global history yet', async () => {
    getHistoryV3.mockResolvedValue({ ...FEED, records: [], totalRecords: 0 });
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/No events have been recorded/i)).toBeTruthy();
    });
  });

  it('surfaces an error when the global feed fails to load', async () => {
    const { ApiError } = await import('@lib/api/client');
    getHistoryV3.mockRejectedValue(new ApiError('internal_error', 'boom', 500));
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/Failed to load history/i)).toBeTruthy();
    });
  });

  it('loads and renders a single node timeline when ?id= is present', async () => {
    searchParams = new URLSearchParams('id=c-1');
    getHistory.mockResolvedValue(EVENTS);
    renderPage();
    await waitFor(() => {
      expect(getHistory).toHaveBeenCalledWith('c-1', expect.anything());
    });
    await waitFor(() => {
      expect(screen.getByText('Grabbed')).toBeTruthy();
      expect(screen.getByText('Imported')).toBeTruthy();
      expect(screen.getByText('Download failed')).toBeTruthy();
      expect(screen.getByText('tracker timeout')).toBeTruthy();
    });
    // The node timeline view does not fetch the global feed.
    expect(getHistoryV3).not.toHaveBeenCalled();
  });

  it('links each node-timeline event to the decision log for its run', async () => {
    searchParams = new URLSearchParams('id=c-1');
    getHistory.mockResolvedValue(EVENTS);
    const { container } = renderPage();
    await waitFor(() => {
      const links = container.querySelectorAll('a[href^="/decision-log?run="]');
      expect(links.length).toBe(EVENTS.length);
      expect((links[0] as HTMLAnchorElement).getAttribute('href')).toContain('run=run-xyz');
    });
  });

  it('hides the raw node-id box behind an Advanced disclosure by default', async () => {
    getHistoryV3.mockResolvedValue(FEED);
    const { container } = renderPage();
    await waitFor(() => {
      expect(getHistoryV3).toHaveBeenCalled();
    });
    // Default (global feed) view: the uuid box is collapsed out of the DOM.
    expect(container.querySelector('input[name="node"]')).toBeNull();
    // ...and revealed only when the user opens the disclosure.
    fireEvent.click(screen.getByText(/Advanced — open a node by id/i));
    expect(container.querySelector('input[name="node"]')).toBeTruthy();
  });

  it('shows the empty state when a content node has no events', async () => {
    searchParams = new URLSearchParams('id=c-empty');
    getHistory.mockResolvedValue([]);
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/No events recorded for this content node yet/i)).toBeTruthy();
    });
  });
});
