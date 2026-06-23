import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

const getHistory = vi.fn();

vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      getHistory: (...a: unknown[]) => getHistory(...a),
    },
  };
});

// History does not read navigation; stub it so AppShell's tree is happy if used.
vi.mock('next/navigation', () => ({
  useSearchParams: () => new URLSearchParams(),
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

const EVENTS = [
  { at: '2024-01-01T10:00:00Z', content_id: 'c-1', run_id: 'run-xyz', event: { event: 'grabbed', grab_id: 'g-1' } },
  { at: '2024-01-01T11:00:00Z', content_id: 'c-1', run_id: 'run-xyz', event: { event: 'imported', grab_id: 'g-1' } },
  { at: '2024-01-01T12:00:00Z', content_id: 'c-1', run_id: 'run-xyz', event: { event: 'download_failed', grab_id: 'g-1', detail: 'tracker timeout' } },
];

function typeAndSubmit(container: HTMLElement, value: string) {
  const input = container.querySelector('input[name="content"]') as HTMLInputElement;
  expect(input).toBeTruthy();
  fireEvent.change(input, { target: { value } });
  const form = input.closest('form') as HTMLFormElement;
  fireEvent.submit(form);
}

describe('History screen', () => {
  beforeEach(() => {
    installMatchMedia();
    window.localStorage.clear();
    getHistory.mockReset();
  });
  afterEach(() => cleanup());

  it('shows the idle no-content state before a query', async () => {
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/No content selected/i)).toBeTruthy();
    });
    expect(getHistory).not.toHaveBeenCalled();
  });

  it('loads and renders the event timeline for a content id', async () => {
    getHistory.mockResolvedValue(EVENTS);
    const { container } = renderPage();
    typeAndSubmit(container, 'c-1');
    await waitFor(() => {
      expect(getHistory).toHaveBeenCalledWith('c-1', expect.anything());
    });
    await waitFor(() => {
      expect(screen.getByText('Grabbed')).toBeTruthy();
      expect(screen.getByText('Imported')).toBeTruthy();
      expect(screen.getByText('Download failed')).toBeTruthy();
      expect(screen.getByText('tracker timeout')).toBeTruthy();
    });
  });

  it('links each event to the decision log for its run', async () => {
    getHistory.mockResolvedValue(EVENTS);
    const { container } = renderPage();
    typeAndSubmit(container, 'c-1');
    await waitFor(() => {
      const links = container.querySelectorAll('a[href^="/decision-log?run="]');
      expect(links.length).toBe(EVENTS.length);
      expect((links[0] as HTMLAnchorElement).getAttribute('href')).toContain('run=run-xyz');
    });
  });

  it('shows the empty state when a content node has no events', async () => {
    getHistory.mockResolvedValue([]);
    const { container } = renderPage();
    typeAndSubmit(container, 'c-empty');
    await waitFor(() => {
      expect(screen.getByText(/No history/i)).toBeTruthy();
    });
  });

  it('surfaces an error when history fails to load', async () => {
    const { ApiError } = await import('@lib/api/client');
    getHistory.mockRejectedValue(new ApiError('internal_error', 'boom', 500));
    const { container } = renderPage();
    typeAndSubmit(container, 'c-1');
    await waitFor(() => {
      expect(screen.getByText(/Failed to load history/i)).toBeTruthy();
    });
  });
});
