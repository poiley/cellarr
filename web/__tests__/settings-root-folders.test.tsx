import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import RootFolders from '@app/settings/_components/RootFolders';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const FOLDERS = [
  { id: 1, path: '/media/movies', accessible: true, freeSpace: 1024 * 1024 * 1024 * 50, unmappedFolders: [] },
];

describe('RootFolders (settings)', () => {
  beforeEach(() => {
    window.matchMedia = vi.fn().mockReturnValue({
      matches: false,
      addEventListener: () => {},
      removeEventListener: () => {},
    }) as never;
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('lists existing root folders with status + free space', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse(FOLDERS));
    const client = new CellarrClient({ fetchImpl });
    render(<RootFolders client={client} />);
    await waitFor(() => expect(screen.getAllByText('/media/movies').length).toBeGreaterThan(0));
    expect(screen.getByText(/accessible/i)).toBeTruthy();
    expect(screen.getByText('50 GB')).toBeTruthy();
  });

  it('POSTs a new root folder', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse([])) // load
      .mockResolvedValueOnce(jsonResponse({ id: 2, path: '/media/tv', accessible: true, freeSpace: 0, unmappedFolders: [] })) // create
      .mockResolvedValueOnce(jsonResponse([])); // reload
    const client = new CellarrClient({ fetchImpl });
    render(<RootFolders client={client} />);
    await waitFor(() => expect(screen.getByLabelText('Root folder path')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Root folder path'), { target: { value: '/media/tv' } });
    fireEvent.click(screen.getByText('Add root folder'));

    await waitFor(() => {
      const post = fetchImpl.mock.calls.find(
        ([url, opts]) => String(url).endsWith('/api/v3/rootfolder') && opts?.method === 'POST'
      );
      expect(post).toBeTruthy();
      const body = JSON.parse((post![1] as RequestInit).body as string);
      expect(body.path).toBe('/media/tv');
    });
  });

  it('confirms then DELETEs a root folder', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(FOLDERS)) // load
      .mockResolvedValueOnce(new Response(null, { status: 200 })) // delete
      .mockResolvedValueOnce(jsonResponse([])); // reload
    const client = new CellarrClient({ fetchImpl });
    render(<RootFolders client={client} />);
    await waitFor(() => expect(screen.getByLabelText('Remove root folder /media/movies')).toBeTruthy());

    fireEvent.click(screen.getByLabelText('Remove root folder /media/movies'));
    expect(fetchImpl.mock.calls.find(([, o]) => o?.method === 'DELETE')).toBeFalsy();
    await waitFor(() => expect(screen.getByRole('alertdialog')).toBeTruthy());

    fireEvent.click(screen.getByRole('button', { name: 'Remove root folder' }));
    await waitFor(() => {
      const del = fetchImpl.mock.calls.find(
        ([url, opts]) => String(url).endsWith('/api/v3/rootfolder/1') && opts?.method === 'DELETE'
      );
      expect(del).toBeTruthy();
    });
  });
});
