import * as React from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render as rtlRender } from '@testing-library/react';

import { ApiError } from '@lib/api/client';
import { ThemeProvider } from '@lib/ThemeProvider';

// The screen embeds the AppShell (theme controller); wrap renders in
// ThemeProvider so its useTheme() resolves.
const render = (ui: React.ReactElement) =>
  rtlRender(<ThemeProvider>{ui}</ThemeProvider>);

const systemStatus = vi.fn();
const getCommands = vi.fn();

vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      systemStatus: (...args: unknown[]) => systemStatus(...args),
      getCommands: (...args: unknown[]) => getCommands(...args),
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

async function loadScreen() {
  const mod = await import('@app/system/page');
  return mod.default;
}

describe('System / Status screen', () => {
  beforeEach(() => {
    systemStatus.mockReset();
    getCommands.mockReset();
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

  it('renders health rows and the raw status JSON', async () => {
    systemStatus.mockResolvedValue(STATUS);
    getCommands.mockResolvedValue(COMMANDS);
    const Screen = await loadScreen();
    const { findByText, container } = render(<Screen />);
    expect(await findByText(/Application/)).toBeTruthy();
    // The raw CodeBlock contains the serialized status.
    expect(container.textContent).toContain('"app_name"');
  });

  it('lists scheduled tasks from /commands', async () => {
    systemStatus.mockResolvedValue(STATUS);
    getCommands.mockResolvedValue(COMMANDS);
    const Screen = await loadScreen();
    const { findByText } = render(<Screen />);
    expect(await findByText(/RssSync/)).toBeTruthy();
    expect(await findByText(/RefreshMetadata/)).toBeTruthy();
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
});
