import * as React from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render as rtlRender, screen, waitFor } from '@testing-library/react';

import { ThemeProvider } from '@lib/ThemeProvider';
import { ToastProvider } from '@app/_lib/ToastProvider';

const render = (ui: React.ReactElement) =>
  rtlRender(
    <ThemeProvider>
      <ToastProvider>{ui}</ToastProvider>
    </ThemeProvider>
  );

const listLogFiles = vi.fn();
const getLogFile = vi.fn();

vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      listLogFiles: (...args: unknown[]) => listLogFiles(...args),
      getLogFile: (...args: unknown[]) => getLogFile(...args),
    },
  };
});

import LogsPage from '@app/logs/page';

const FILES = [
  {
    id: 1,
    filename: 'cellarr.txt',
    lastWriteTime: '2026-06-20T12:00:00Z',
    contentsUrl: '/api/v3/log/file/cellarr.txt',
    size: 2048,
  },
];

const LOG_TEXT = [
  '2026-06-20 INFO starting up',
  '2026-06-20 WARN disk getting full',
  '2026-06-20 ERROR failed to write file',
  '2026-06-20 DEBUG verbose detail',
].join('\n');

describe('Logs screen', () => {
  beforeEach(() => {
    window.matchMedia = vi.fn().mockReturnValue({
      matches: false,
      addEventListener: () => {},
      removeEventListener: () => {},
    }) as never;
    listLogFiles.mockReset();
    getLogFile.mockReset();
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('lists files and renders the selected log lines in a terminal view', async () => {
    listLogFiles.mockResolvedValue(FILES);
    getLogFile.mockResolvedValue(LOG_TEXT);

    render(<LogsPage />);

    // File appears in the list.
    await waitFor(() => expect(screen.getByText('cellarr.txt')).toBeTruthy());
    // Auto-selected newest file -> its lines render.
    await waitFor(() => expect(screen.getByText(/starting up/)).toBeTruthy());
    expect(screen.getByText(/disk getting full/)).toBeTruthy();
    expect(screen.getByText(/failed to write file/)).toBeTruthy();

    // The tail was requested with the default line count.
    const call = getLogFile.mock.calls.at(-1);
    expect(call?.[0]).toBe('cellarr.txt');
    expect(call?.[1]).toBe(500);
  });

  it('filters lines by level', async () => {
    listLogFiles.mockResolvedValue(FILES);
    getLogFile.mockResolvedValue(LOG_TEXT);

    render(<LogsPage />);
    await waitFor(() => expect(screen.getByText(/starting up/)).toBeTruthy());

    // Open the level Select and choose ERROR.
    fireEvent.click(screen.getByText('All'));
    await waitFor(() => expect(screen.getByRole('option', { name: 'ERROR' })).toBeTruthy());
    fireEvent.click(screen.getByRole('option', { name: 'ERROR' }));

    await waitFor(() => {
      // INFO/WARN/DEBUG lines are filtered out; only ERROR remains.
      expect(screen.queryByText(/starting up/)).toBeNull();
      expect(screen.queryByText(/disk getting full/)).toBeNull();
      expect(screen.getByText(/failed to write file/)).toBeTruthy();
    });
  });

  it('refetches the tail when Refresh is clicked', async () => {
    listLogFiles.mockResolvedValue(FILES);
    getLogFile.mockResolvedValue(LOG_TEXT);

    render(<LogsPage />);
    await waitFor(() => expect(screen.getByText(/starting up/)).toBeTruthy());

    const before = getLogFile.mock.calls.length;
    fireEvent.click(screen.getByText('Refresh'));
    await waitFor(() => expect(getLogFile.mock.calls.length).toBeGreaterThan(before));
  });
});
