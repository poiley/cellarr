import * as React from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render as rtlRender, waitFor } from '@testing-library/react';

import { ApiError } from '@lib/api/client';
import { ThemeProvider } from '@lib/ThemeProvider';
import { ToastProvider } from '@app/_lib/ToastProvider';

// The screen embeds the AppShell (theme controller) + uses the shared toast
// hook; wrap renders in both providers so useTheme()/useToast() resolve.
const render = (ui: React.ReactElement) =>
  rtlRender(
    <ThemeProvider>
      <ToastProvider>{ui}</ToastProvider>
    </ThemeProvider>
  );

const systemStatus = vi.fn();
const getCommands = vi.fn();
const requestV3 = vi.fn();
const health = vi.fn();

vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      systemStatus: (...args: unknown[]) => systemStatus(...args),
      getCommands: (...args: unknown[]) => getCommands(...args),
      health: (...args: unknown[]) => health(...args),
      // The System screen reaches the scheduler surface + 'Run now' command
      // through the client's generic requestV3 escape hatch.
      requestV3: (...args: unknown[]) => requestV3(...args),
    },
  };
});

const STATUS = {
  app_name: 'cellarr',
  version: '0.1.0',
  auth_enabled: false,
  library_count: 2,
  indexer_count: 1,
  download_client_count: 1,
};

const COMMANDS = [
  { name: 'RssSync', description: 'Sync the latest releases.' },
  { name: 'RefreshMetadata', description: 'Refresh metadata.' },
];

const TASKS = [
  {
    id: 1,
    name: 'RSS Sync',
    taskName: 'RssSync',
    interval: 15,
    nextExecution: new Date(Date.now() + 5 * 60_000).toISOString(),
    lastExecution: new Date(Date.now() - 10 * 60_000).toISOString(),
    lastDuration: '00:00:00',
    lastStatus: 'ok',
  },
  {
    id: 2,
    name: 'Refresh Metadata',
    taskName: 'RefreshMetadata',
    interval: 720,
    nextExecution: new Date(Date.now() + 60 * 60_000).toISOString(),
    lastExecution: null,
    lastDuration: '00:00:00',
    lastStatus: 'ok',
  },
];

async function loadScreen() {
  const mod = await import('@app/system/page');
  return mod.default;
}

describe('System / Status screen', () => {
  beforeEach(() => {
    systemStatus.mockReset();
    getCommands.mockReset();
    requestV3.mockReset();
    health.mockReset();
    // Default: an empty health list (the 'all healthy' state). Tests override.
    health.mockResolvedValue([]);
    // Default: route GET /system/task and POST /command. Individual tests
    // override as needed.
    requestV3.mockImplementation((path: string, opts?: { method?: string }) => {
      if (path === '/system/task') return Promise.resolve(TASKS);
      if (path === '/command' && opts?.method === 'POST')
        return Promise.resolve({ id: 'cmd-1', status: 'queued' });
      return Promise.resolve(undefined);
    });
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

  it('shows a loading state before status resolves', async () => {
    systemStatus.mockReturnValue(new Promise(() => {}));
    getCommands.mockReturnValue(new Promise(() => {}));
    const Screen = await loadScreen();
    const { getByText } = render(<Screen />);
    expect(getByText(/Loading system status/i)).toBeTruthy();
  });

  it('renders health rows and the raw status JSON behind the disclosure', async () => {
    systemStatus.mockResolvedValue(STATUS);
    getCommands.mockResolvedValue(COMMANDS);
    const Screen = await loadScreen();
    const { findByText, container } = render(<Screen />);
    expect(await findByText(/Application/)).toBeTruthy();
    // Raw JSON is collapsed by default; expand the Raw / Advanced disclosure.
    expect(container.textContent).not.toContain('"app_name"');
    fireEvent.click(await findByText('Raw / Advanced'));
    expect(container.textContent).toContain('"app_name"');
  });

  it('lists scheduled tasks with their schedule metadata', async () => {
    systemStatus.mockResolvedValue(STATUS);
    getCommands.mockResolvedValue(COMMANDS);
    const Screen = await loadScreen();
    const { findByText } = render(<Screen />);
    // The rich scheduler tasks drive the table when present.
    expect(await findByText(/RSS Sync/)).toBeTruthy();
    expect(await findByText(/Refresh Metadata/)).toBeTruthy();
    // Cadence is surfaced (15 min for RSS Sync).
    expect(await findByText(/15 min/)).toBeTruthy();
  });

  it('falls back to the native command catalogue when the scheduler is absent', async () => {
    systemStatus.mockResolvedValue(STATUS);
    getCommands.mockResolvedValue(COMMANDS);
    requestV3.mockImplementation((path: string) => {
      if (path === '/system/task') return Promise.reject(new ApiError('http_error', 'no', 404));
      return Promise.resolve({ id: 'cmd-1', status: 'queued' });
    });
    const Screen = await loadScreen();
    const { findByText } = render(<Screen />);
    // With no scheduler tasks, rows are synthesized from /commands names.
    expect(await findByText(/RssSync/)).toBeTruthy();
    expect(await findByText(/RefreshMetadata/)).toBeTruthy();
  });

  it("'Run now' POSTs the task command and confirms via toast", async () => {
    systemStatus.mockResolvedValue(STATUS);
    getCommands.mockResolvedValue(COMMANDS);
    const Screen = await loadScreen();
    const { findAllByText, findByText } = render(<Screen />);
    const runButtons = await findAllByText('Run now');
    expect(runButtons.length).toBeGreaterThan(0);
    fireEvent.click(runButtons[0]);
    await waitFor(() =>
      expect(
        requestV3.mock.calls.some(
          ([path, opts]) =>
            path === '/command' &&
            (opts as { method?: string; body?: { name?: string } })?.method === 'POST' &&
            (opts as { body?: { name?: string } })?.body?.name === 'RssSync'
        )
      ).toBe(true)
    );
    expect(await findByText(/queued\./i)).toBeTruthy();
  });

  it('keeps the raw status JSON behind a Raw / Advanced disclosure with a copy control', async () => {
    systemStatus.mockResolvedValue(STATUS);
    getCommands.mockResolvedValue(COMMANDS);
    const Screen = await loadScreen();
    const { findByText, getByText } = render(<Screen />);
    // The disclosure title is present; expanding it reveals copy + the JSON.
    const disclosure = await findByText('Raw / Advanced');
    fireEvent.click(disclosure);
    expect(getByText('Copy')).toBeTruthy();
  });

  it('warns when no indexers are configured', async () => {
    systemStatus.mockResolvedValue({ ...STATUS, indexer_count: 0 });
    getCommands.mockResolvedValue(COMMANDS);
    const Screen = await loadScreen();
    const { findByText } = render(<Screen />);
    expect(await findByText(/No indexers configured/i)).toBeTruthy();
  });

  it('shows an offline Message when the daemon is unreachable', async () => {
    systemStatus.mockRejectedValue(new ApiError('network_error', 'down', 0));
    getCommands.mockRejectedValue(new ApiError('network_error', 'down', 0));
    const Screen = await loadScreen();
    const { findByText } = render(<Screen />);
    expect(await findByText(/daemon is not reachable/i)).toBeTruthy();
  });

  it('surfaces a non-network error as an alert', async () => {
    systemStatus.mockRejectedValue(new ApiError('internal_error', 'kaboom', 500));
    getCommands.mockResolvedValue(COMMANDS);
    const Screen = await loadScreen();
    const { findByText } = render(<Screen />);
    expect(await findByText(/Could not reach the API/i)).toBeTruthy();
  });

  it('shows an all-healthy empty state when there are no health checks', async () => {
    systemStatus.mockResolvedValue(STATUS);
    getCommands.mockResolvedValue(COMMANDS);
    health.mockResolvedValue([]);
    const Screen = await loadScreen();
    const { findByText } = render(<Screen />);
    expect(await findByText(/All health checks passed/i)).toBeTruthy();
  });

  it('renders each health check with its severity and message', async () => {
    systemStatus.mockResolvedValue(STATUS);
    getCommands.mockResolvedValue(COMMANDS);
    health.mockResolvedValue([
      { source: 'IndexerCheck', type: 'warning', message: 'No indexers are enabled' },
      { source: 'RootFolderCheck', type: 'error', message: 'Root folder is not writable' },
    ]);
    const Screen = await loadScreen();
    const { findByText, getAllByRole } = render(<Screen />);
    expect(await findByText(/No indexers are enabled/)).toBeTruthy();
    expect(await findByText(/Root folder is not writable/)).toBeTruthy();
    // Both a warning and an error severity badge appear.
    expect(await findByText('warning')).toBeTruthy();
    expect(await findByText('error')).toBeTruthy();
    expect(getAllByRole('listitem').length).toBe(2);
  });
});
