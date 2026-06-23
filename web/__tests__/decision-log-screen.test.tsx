import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

// The Decision-log screen reads the singleton API client and Next's
// useSearchParams; mock both for jsdom.

const getDecisionLog = vi.fn();
let searchParams = new URLSearchParams();

vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      getDecisionLog: (...a: unknown[]) => getDecisionLog(...a),
    },
  };
});

vi.mock('next/navigation', () => ({
  useSearchParams: () => searchParams,
}));

import DecisionLogPage from '@app/decision-log/page';
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
      <DecisionLogPage />
    </ThemeProvider>
  );
}

const REJECT_RECORD = {
  at: '2024-01-01T10:00:00Z',
  run_id: 'run-abc',
  transition: { from: 'decide', to: 'rejected', kind: 'reject' },
  decision: {
    content_ref: { id: 'c-1', library_id: 'lib-tv', media_type: 'tv', coords: { type: 'episode', season: 1, episode: 2 } },
    release: { indexer_id: 'idx', title: 'Show.S01E02.1080p.WEB-DL', download_url: 'magnet:x', protocol: 'torrent', size: 2147483648 },
    parsed: { raw_title: 'Show.S01E02.1080p.WEB-DL', resolution: '1080p', source: 'web-dl', confidence: { resolution: 1, source: 0.8 } },
    verdict: { verdict: 'reject', reason: { reason: 'not_an_upgrade' } },
  },
};

const GRAB_RECORD = {
  at: '2024-01-01T09:00:00Z',
  run_id: 'run-abc',
  transition: { from: 'decide', to: 'grab', kind: 'advance' },
  decision: {
    content_ref: { id: 'c-1', library_id: 'lib-tv', media_type: 'tv', coords: { type: 'episode', season: 1, episode: 2 } },
    release: { indexer_id: 'idx', title: 'Show.S01E02.2160p.BluRay', download_url: 'magnet:y', protocol: 'usenet' },
    verdict: { verdict: 'grab', score: { quality_rank: 7, custom_format_score: 150 } },
  },
};

describe('Decision-log screen', () => {
  beforeEach(() => {
    installMatchMedia();
    window.localStorage.clear();
    searchParams = new URLSearchParams();
    getDecisionLog.mockReset();
  });
  afterEach(() => cleanup());

  it('shows the idle no-run state with no run param', async () => {
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/No run selected/i)).toBeTruthy();
    });
    expect(getDecisionLog).not.toHaveBeenCalled();
  });

  it('auto-loads a run from the ?run= query param', async () => {
    getDecisionLog.mockResolvedValue([GRAB_RECORD, REJECT_RECORD]);
    searchParams = new URLSearchParams('run=run-abc');
    renderPage();
    await waitFor(() => {
      expect(getDecisionLog).toHaveBeenCalledWith('run-abc', expect.anything());
    });
    await waitFor(() => {
      expect(screen.getByText(/2 records/i)).toBeTruthy();
    });
  });

  it('renders verdict pills and the CF-score breakdown for a grab', async () => {
    getDecisionLog.mockResolvedValue([GRAB_RECORD]);
    searchParams = new URLSearchParams('run=run-abc');
    renderPage();
    await waitFor(() => {
      expect(screen.getByText('GRAB')).toBeTruthy();
    });
    // First record auto-expands; its CF-score breakdown shows the signed score.
    await waitFor(() => {
      expect(screen.getAllByText('+150').length).toBeGreaterThan(0);
    });
  });

  it('shows the rejection reason for a reject verdict', async () => {
    getDecisionLog.mockResolvedValue([REJECT_RECORD]);
    searchParams = new URLSearchParams('run=run-abc');
    renderPage();
    await waitFor(() => {
      expect(screen.getByText('REJECT')).toBeTruthy();
    });
    await waitFor(() => {
      expect(screen.getAllByText(/equal or better file already exists/i).length).toBeGreaterThan(0);
    });
  });

  it('loads a run typed into the input', async () => {
    getDecisionLog.mockResolvedValue([GRAB_RECORD]);
    const { container } = renderPage();
    const input = container.querySelector('input[name="run"]') as HTMLInputElement;
    expect(input).toBeTruthy();
    fireEvent.change(input, { target: { value: 'typed-run' } });
    const form = input.closest('form') as HTMLFormElement;
    fireEvent.submit(form);
    await waitFor(() => {
      expect(getDecisionLog).toHaveBeenCalledWith('typed-run', expect.anything());
    });
  });

  it('shows the empty state when a run has no records', async () => {
    getDecisionLog.mockResolvedValue([]);
    searchParams = new URLSearchParams('run=empty');
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/No records/i)).toBeTruthy();
    });
  });

  it('surfaces an error when the log fails to load', async () => {
    const { ApiError } = await import('@lib/api/client');
    getDecisionLog.mockRejectedValue(new ApiError('internal_error', 'boom', 500));
    searchParams = new URLSearchParams('run=broken');
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/Failed to load the decision log/i)).toBeTruthy();
    });
  });
});
