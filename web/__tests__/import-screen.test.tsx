import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { ThemeProvider } from '@lib/ThemeProvider';
import { ModalProvider } from '@components/page/ModalContext';
import { HotkeysProvider } from '@modules/hotkeys';

// The Manual Import screen drives the shared client's v3 escape hatch (scan +
// commit) and the library list readers (re-map picker seed). Mock them so the
// component test is hermetic.
const requestV3 = vi.fn();
const listMovies = vi.fn();
const listSeries = vi.fn();
vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      requestV3: (...args: unknown[]) => requestV3(...args),
      listMovies: (...args: unknown[]) => listMovies(...args),
      listSeries: (...args: unknown[]) => listSeries(...args),
    },
  };
});

import ImportPage from '@app/import/page';
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
      <HotkeysProvider>
        <ToastProvider>
          <ModalProvider>
            <ImportPage />
          </ModalProvider>
        </ToastProvider>
      </HotkeysProvider>
    </ThemeProvider>
  );
}

// A scan row identified onto a content node and not rejected.
const goodRow = {
  path: '/downloads/Blade Runner 2049 1080p.mkv',
  name: 'Blade Runner 2049 1080p.mkv',
  size: 8_000_000_000,
  parsedTitle: 'Blade Runner 2049',
  quality: { quality: { id: 9, name: 'Bluray-1080p' } },
  contentId: '11111111-1111-1111-1111-111111111111',
  rejected: false,
  rejections: [],
};

// A scan row the daemon would reject (e.g. unknown quality).
const rejectedRow = {
  path: '/downloads/mystery.avi',
  name: 'mystery.avi',
  size: 700_000_000,
  parsedTitle: 'mystery',
  quality: { quality: { id: 0, name: 'Unknown' } },
  rejected: true,
  rejections: [{ reason: 'Unknown quality' }],
};

describe('Manual Import screen', () => {
  beforeEach(() => {
    requestV3.mockReset();
    listMovies.mockReset();
    listSeries.mockReset();
    listMovies.mockResolvedValue([{ id: 'm-1', title: 'Blade Runner 2049', year: 2017 }]);
    listSeries.mockResolvedValue([]);
    window.localStorage.clear();
    document.body.className = '';
    installMatchMedia();
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('auto-scans the library on mount and surfaces untracked files', async () => {
    requestV3.mockImplementation((path: string) => {
      if (path === '/manualImport') return Promise.resolve([goodRow]);
      return Promise.resolve(undefined);
    });
    renderPage();
    // The mount scan hits the library roots (no `folder` query) so untracked
    // in-place files auto-surface without the user pointing at a folder.
    await waitFor(() =>
      expect(requestV3).toHaveBeenCalledWith(
        '/manualImport',
        expect.objectContaining({ query: {} })
      )
    );
    await waitFor(() => expect(screen.getByText('Blade Runner 2049 1080p.mkv')).toBeTruthy());
  });

  it('reports an empty library when nothing untracked is on disk', async () => {
    requestV3.mockResolvedValue([]);
    renderPage();
    await waitFor(() =>
      expect(screen.getByText(/No untracked files under the library/i)).toBeTruthy()
    );
  });

  it('scans a folder and renders candidate rows', async () => {
    requestV3.mockImplementation((path: string) => {
      if (path === '/manualImport') return Promise.resolve([goodRow, rejectedRow]);
      return Promise.resolve(undefined);
    });
    const { container } = renderPage();

    const input = container.querySelector('input[name="import-folder"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: '/downloads' } });
    fireEvent.click(screen.getByText('Scan'));

    await waitFor(() =>
      expect(requestV3).toHaveBeenCalledWith(
        '/manualImport',
        expect.objectContaining({ query: { folder: '/downloads' } })
      )
    );
    await waitFor(() => expect(screen.getByText('Blade Runner 2049 1080p.mkv')).toBeTruthy());
    // Parsed title + quality + rejection reason all surface.
    expect(screen.getByText('Bluray-1080p')).toBeTruthy();
    expect(screen.getByText(/Unknown quality/)).toBeTruthy();
  });

  it('pre-selects only identified, non-rejected rows for import', async () => {
    requestV3.mockImplementation((path: string) => {
      if (path === '/manualImport') return Promise.resolve([goodRow, rejectedRow]);
      return Promise.resolve(undefined);
    });
    const { container } = renderPage();
    const input = container.querySelector('input[name="import-folder"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: '/downloads' } });
    fireEvent.click(screen.getByText('Scan'));

    // One of two rows is selected by default (the good one).
    await waitFor(() => expect(screen.getByText('1 of 2 selected')).toBeTruthy());
  });

  it('commits the included files and shows a success toast', async () => {
    requestV3.mockImplementation((path: string, opts?: { method?: string }) => {
      if (path === '/manualImport' && opts?.method === 'POST') {
        return Promise.resolve({
          imported: [
            {
              sourcePath: goodRow.path,
              destinationPath: '/movies/Blade Runner 2049/Blade Runner 2049.mkv',
              contentId: goodRow.contentId,
            },
          ],
          errors: [],
        });
      }
      if (path === '/manualImport') return Promise.resolve([goodRow]);
      return Promise.resolve(undefined);
    });
    const { container } = renderPage();
    const input = container.querySelector('input[name="import-folder"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: '/downloads' } });
    fireEvent.click(screen.getByText('Scan'));

    await waitFor(() => expect(screen.getByText(/Import 1/)).toBeTruthy());
    fireEvent.click(screen.getByText(/Import 1/));

    // The commit POST carries the chosen file path + content id.
    await waitFor(() =>
      expect(requestV3).toHaveBeenCalledWith(
        '/manualImport',
        expect.objectContaining({
          method: 'POST',
          body: { files: [{ path: goodRow.path, contentId: goodRow.contentId }] },
        })
      )
    );
    await waitFor(() => expect(screen.getByText(/Imported/)).toBeTruthy());
  });

  it('does not commit until Import is pressed (scan moves nothing)', async () => {
    requestV3.mockImplementation((path: string) => {
      if (path === '/manualImport') return Promise.resolve([goodRow]);
      return Promise.resolve(undefined);
    });
    const { container } = renderPage();
    const input = container.querySelector('input[name="import-folder"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: '/downloads' } });
    fireEvent.click(screen.getByText('Scan'));
    await waitFor(() => expect(screen.getByText('Blade Runner 2049 1080p.mkv')).toBeTruthy());

    // Only the GET scan has fired; no POST commit happened.
    const postCalls = requestV3.mock.calls.filter(
      (c) => (c[1] as { method?: string } | undefined)?.method === 'POST'
    );
    expect(postCalls.length).toBe(0);
  });

  it('reports an empty result when the folder has no importable files', async () => {
    requestV3.mockResolvedValue([]);
    const { container } = renderPage();
    const input = container.querySelector('input[name="import-folder"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: '/empty' } });
    fireEvent.click(screen.getByText('Scan'));
    await waitFor(() => expect(screen.getByText(/No importable files found/i)).toBeTruthy());
  });

  it('surfaces a scan error banner on a 400', async () => {
    requestV3.mockRejectedValue(new Error('boom'));
    const { container } = renderPage();
    const input = container.querySelector('input[name="import-folder"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: '/x' } });
    fireEvent.click(screen.getByText('Scan'));
    await waitFor(() => expect(screen.getByText(/Scan failed/i)).toBeTruthy());
  });
});
