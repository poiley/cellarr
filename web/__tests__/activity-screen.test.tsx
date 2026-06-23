import * as React from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, render as rtlRender, waitFor } from '@testing-library/react';

import { ApiError } from '@lib/api/client';
import { ThemeProvider } from '@lib/ThemeProvider';

// The screen embeds the AppShell (which uses the theme controller); wrap renders
// in ThemeProvider so its useTheme() resolves.
const render = (ui: React.ReactElement) =>
  rtlRender(<ThemeProvider>{ui}</ThemeProvider>);

// --- mock the API client the screen imports ---------------------------------
const getQueue = vi.fn();
vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: { getQueue: (...args: unknown[]) => getQueue(...args) },
    resolveBaseUrl: () => '',
  };
});

// --- a controllable EventSource stub ----------------------------------------
type Listener = (ev: MessageEvent) => void;

class FakeEventSource {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSED = 2;
  static instances: FakeEventSource[] = [];

  url: string;
  readyState = FakeEventSource.CONNECTING;
  closed = false;
  private listeners: Record<string, Listener[]> = {};

  constructor(url: string) {
    this.url = url;
    FakeEventSource.instances.push(this);
  }

  addEventListener(type: string, cb: Listener) {
    (this.listeners[type] ||= []).push(cb);
  }

  hasListener(type: string) {
    return (this.listeners[type]?.length ?? 0) > 0;
  }

  close() {
    this.closed = true;
    this.readyState = FakeEventSource.CLOSED;
  }

  emit(type: string, data?: unknown) {
    this.readyState = type === 'open' ? FakeEventSource.OPEN : this.readyState;
    const ev = { data: data !== undefined ? JSON.stringify(data) : '' } as MessageEvent;
    for (const cb of this.listeners[type] ?? []) cb(ev);
  }
}

async function loadScreen() {
  const mod = await import('@app/activity/page');
  return mod.default;
}

describe('Activity / Queue screen', () => {
  beforeEach(() => {
    getQueue.mockReset();
    FakeEventSource.instances = [];
    (globalThis as Record<string, unknown>).EventSource =
      FakeEventSource as unknown as typeof EventSource;
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

  it('shows a loading indicator before the queue resolves', async () => {
    let resolve: (v: unknown) => void = () => {};
    getQueue.mockReturnValue(new Promise((r) => (resolve = r)));
    const Screen = await loadScreen();
    const { getByText } = render(<Screen />);
    expect(getByText(/Loading the queue/i)).toBeTruthy();
    await act(async () => {
      resolve([]);
    });
  });

  it('renders an empty state when the queue is empty', async () => {
    getQueue.mockResolvedValue([]);
    const Screen = await loadScreen();
    const { findByText } = render(<Screen />);
    expect(await findByText(/Nothing in the queue/i)).toBeTruthy();
  });

  it('renders queue rows from the snapshot', async () => {
    getQueue.mockResolvedValue([
      { id: 'job-1', command: 'RssSync', state: 'running', attempts: 1 },
    ]);
    const Screen = await loadScreen();
    const { findAllByText } = render(<Screen />);
    expect((await findAllByText(/RssSync/)).length).toBeGreaterThan(0);
  });

  it('shows a connecting badge until the stream opens, then live', async () => {
    getQueue.mockResolvedValue([]);
    const Screen = await loadScreen();
    const { findByText } = render(<Screen />);
    expect(await findByText(/connecting/i)).toBeTruthy();
    await act(async () => {
      FakeEventSource.instances[0].emit('open');
    });
    expect(await findByText(/live/i)).toBeTruthy();
  });

  it('merges a live queue_progress frame into the view', async () => {
    getQueue.mockResolvedValue([
      { id: 'job-1', command: 'ManualSearch', state: 'queued', attempts: 0 },
    ]);
    const Screen = await loadScreen();
    const { findAllByText } = render(<Screen />);
    await findAllByText(/ManualSearch/);
    await waitFor(() => expect(FakeEventSource.instances.length).toBe(1));
    const es = FakeEventSource.instances[0];
    await waitFor(() => expect(es.hasListener('queue_progress')).toBe(true));
    await act(async () => {
      es.emit('queue_progress', {
        grab_id: 'job-1',
        status: 'downloading',
        progress: 0.5,
      });
    });
    expect((await findAllByText(/downloading/)).length).toBeGreaterThan(0);
    expect((await findAllByText(/50%/)).length).toBeGreaterThan(0);
  });

  it('closes the EventSource on unmount', async () => {
    getQueue.mockResolvedValue([]);
    const Screen = await loadScreen();
    const { unmount } = render(<Screen />);
    await waitFor(() => expect(FakeEventSource.instances.length).toBe(1));
    unmount();
    expect(FakeEventSource.instances[0].closed).toBe(true);
  });

  it('surfaces a non-network API error as an alert', async () => {
    getQueue.mockRejectedValue(new ApiError('internal_error', 'boom', 500));
    const Screen = await loadScreen();
    const { findByText } = render(<Screen />);
    expect(await findByText(/Could not load the queue/i)).toBeTruthy();
  });
});
