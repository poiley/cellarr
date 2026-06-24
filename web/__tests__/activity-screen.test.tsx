import * as React from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, render as rtlRender, waitFor } from '@testing-library/react';

import { ApiError } from '@lib/api/client';
import { ThemeProvider } from '@lib/ThemeProvider';
import type { DomainEvent } from '@lib/api/types';

// The screen embeds the AppShell (which uses the theme controller); wrap renders
// in ThemeProvider so its useTheme() resolves.
const render = (ui: React.ReactElement) =>
  rtlRender(<ThemeProvider>{ui}</ThemeProvider>);

// --- mock the API client the screen imports ---------------------------------
// The new Activity screen reads the v3 queue + blocklist via api.poll, and the
// lifecycle via api.openStream. We drive both through controllable fakes.
const getQueueV3 = vi.fn();
const getBlocklist = vi.fn();

interface PollOpts<T> {
  intervalMs?: number;
  immediate?: boolean;
  onData: (d: T) => void;
  onError?: (e: unknown) => void;
}
let lastPollOnError: ((e: unknown) => void) | undefined;

interface StreamHandlers {
  onOpen?: () => void;
  onError?: (e: unknown) => void;
  on?: Partial<{ [K in DomainEvent['type']]: (ev: Extract<DomainEvent, { type: K }>) => void }>;
}
const streamHandlers: StreamHandlers[] = [];

vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      getQueueV3: (...args: unknown[]) => getQueueV3(...args),
      getBlocklist: (...args: unknown[]) => getBlocklist(...args),
      // Run the fetcher once immediately (the screen passes a Promise.all
      // fetcher); funnel its result/error into onData/onError.
      poll<T>(fetcher: (signal: AbortSignal) => Promise<T>, options: PollOpts<T>) {
        lastPollOnError = options.onError;
        const controller = new AbortController();
        void fetcher(controller.signal)
          .then((d) => options.onData(d))
          .catch((e) => options.onError?.(e));
        return { stop: () => controller.abort() };
      },
      openStream(options: StreamHandlers) {
        streamHandlers.push(options);
        return { close: () => {} };
      },
    },
    resolveBaseUrl: () => '',
  };
});

function page<T>(records: T[]) {
  return { page: 1, pageSize: Math.max(records.length, 1), totalRecords: records.length, records };
}

function emit<K extends DomainEvent['type']>(
  type: K,
  ev: Extract<DomainEvent, { type: K }>
) {
  for (const h of streamHandlers) h.on?.[type]?.(ev as never);
}

async function loadScreen() {
  const mod = await import('@app/activity/page');
  return mod.default;
}

describe('Activity screen', () => {
  beforeEach(() => {
    getQueueV3.mockReset();
    getBlocklist.mockReset();
    streamHandlers.length = 0;
    lastPollOnError = undefined;
    (globalThis as Record<string, unknown>).EventSource = class {} as unknown as typeof EventSource;
    // BarProgress observes its container width; jsdom has no ResizeObserver.
    (globalThis as Record<string, unknown>).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
    window.localStorage.clear();
    document.body.className = '';
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
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('shows a loading indicator before the snapshot resolves', async () => {
    getQueueV3.mockReturnValue(new Promise(() => {}));
    getBlocklist.mockReturnValue(new Promise(() => {}));
    const Screen = await loadScreen();
    const { getByText } = render(<Screen />);
    expect(getByText(/Loading activity/i)).toBeTruthy();
  });

  it('keeps the scheduled cron jobs in their own labelled section, not downloads', async () => {
    getQueueV3.mockResolvedValue(
      page([
        { id: 'j1', title: 'MissingItemSearch', status: 'scheduled', protocol: 'unknown' },
        { id: 'j2', title: 'RssSync', status: 'scheduled', protocol: 'unknown' },
      ])
    );
    getBlocklist.mockResolvedValue(page([]));
    const Screen = await loadScreen();
    const { findByText, getAllByText } = render(<Screen />);
    // The "Scheduled tasks" heading appears and the cron jobs are listed.
    expect(await findByText(/Scheduled tasks/i)).toBeTruthy();
    expect(getAllByText(/MissingItemSearch/).length).toBeGreaterThan(0);
    // Downloads section reports nothing active (cron jobs are NOT downloads).
    expect(await findByText(/No active downloads/i)).toBeTruthy();
  });

  it('shows a real download with progress, separate from scheduled tasks', async () => {
    getQueueV3.mockResolvedValue(
      page([
        { id: 'g1', title: 'The Matrix 1999', status: 'downloading', protocol: 'torrent', size: 100, sizeleft: 25 },
        { id: 'j1', title: 'RssSync', status: 'scheduled', protocol: 'unknown' },
      ])
    );
    getBlocklist.mockResolvedValue(page([]));
    const Screen = await loadScreen();
    const { findAllByText, findByText } = render(<Screen />);
    expect((await findAllByText(/The Matrix 1999/)).length).toBeGreaterThan(0);
    expect((await findAllByText(/downloading/)).length).toBeGreaterThan(0);
    expect(await findByText(/Scheduled tasks/i)).toBeTruthy();
  });

  it('merges a live queue_progress frame into the download view', async () => {
    getQueueV3.mockResolvedValue(
      page([{ id: 'g1', title: 'Blade Runner', status: 'grabbed', protocol: 'torrent' }])
    );
    getBlocklist.mockResolvedValue(page([]));
    const Screen = await loadScreen();
    const { findAllByText } = render(<Screen />);
    await findAllByText(/Blade Runner/);
    await waitFor(() => expect(streamHandlers.length).toBeGreaterThan(0));
    await act(async () => {
      emit('queue_progress', { type: 'queue_progress', grab_id: 'g1', status: 'downloading', progress: 0.5 });
    });
    expect((await findAllByText(/downloading/)).length).toBeGreaterThan(0);
  });

  it('surfaces a blocklisted release in the self-heal section', async () => {
    getQueueV3.mockResolvedValue(page([]));
    getBlocklist.mockResolvedValue(
      page([
        { id: 7, sourceTitle: 'Dune.2021.BAD', date: 1700000000, indexer: 'demo', message: 'hash mismatch' },
      ])
    );
    const Screen = await loadScreen();
    const { findByText, getAllByText } = render(<Screen />);
    expect(await findByText(/Self-heal/i)).toBeTruthy();
    expect(getAllByText(/Dune.2021.BAD/).length).toBeGreaterThan(0);
    expect(getAllByText(/blocklisted/i).length).toBeGreaterThan(0);
  });

  it('shows a recovery decision pushed over the stream', async () => {
    getQueueV3.mockResolvedValue(page([]));
    getBlocklist.mockResolvedValue(page([]));
    const Screen = await loadScreen();
    const { findByText } = render(<Screen />);
    await findByText(/Downloads/i);
    await waitFor(() => expect(streamHandlers.length).toBeGreaterThan(0));
    await act(async () => {
      emit('decision_logged', { type: 'decision_logged', run_id: 'r1', note: 'Grabbed next candidate' });
    });
    expect(await findByText(/Grabbed next candidate/)).toBeTruthy();
  });

  it('surfaces a non-network API error as an alert', async () => {
    getQueueV3.mockRejectedValue(new ApiError('internal_error', 'boom', 500));
    getBlocklist.mockRejectedValue(new ApiError('internal_error', 'boom', 500));
    const Screen = await loadScreen();
    const { findByText } = render(<Screen />);
    expect(await findByText(/Could not load activity/i)).toBeTruthy();
  });
});
