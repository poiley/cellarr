import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import RemotePathMappings from '@app/settings/_components/RemotePathMappings';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const MAPPINGS = [
  { id: 3, host: 'seedbox', remotePath: '/downloads/', localPath: '/mnt/dl/' },
];

describe('RemotePathMappings (settings)', () => {
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

  it('lists existing mappings', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse(MAPPINGS));
    const client = new CellarrClient({ fetchImpl });
    render(<RemotePathMappings client={client} />);
    await waitFor(() => expect(screen.getByText('seedbox')).toBeTruthy());
    expect(screen.getAllByText('/downloads/').length).toBeGreaterThan(0);
  });

  it('POSTs a new mapping', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse([])) // load
      .mockResolvedValueOnce(jsonResponse({ id: 4 })) // create
      .mockResolvedValueOnce(jsonResponse([])); // reload
    const client = new CellarrClient({ fetchImpl });
    render(<RemotePathMappings client={client} />);
    await waitFor(() => expect(screen.getByLabelText('Mapping host')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Mapping host'), { target: { value: 'nas' } });
    fireEvent.change(screen.getByLabelText('Mapping remote path'), { target: { value: '/r/' } });
    fireEvent.change(screen.getByLabelText('Mapping local path'), { target: { value: '/l/' } });
    fireEvent.click(screen.getByText('Add mapping'));

    await waitFor(() => {
      const post = fetchImpl.mock.calls.find(
        ([url, opts]) => String(url).endsWith('/api/v3/remotepathmapping') && opts?.method === 'POST'
      );
      expect(post).toBeTruthy();
      const body = JSON.parse((post![1] as RequestInit).body as string);
      expect(body.host).toBe('nas');
      expect(body.remotePath).toBe('/r/');
      expect(body.localPath).toBe('/l/');
    });
  });

  it('edits an existing mapping with a PUT', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(MAPPINGS)) // load
      .mockResolvedValueOnce(jsonResponse({ id: 3 })) // put
      .mockResolvedValueOnce(jsonResponse([])); // reload
    const client = new CellarrClient({ fetchImpl });
    render(<RemotePathMappings client={client} />);
    await waitFor(() => expect(screen.getByLabelText('Edit mapping for seedbox')).toBeTruthy());

    fireEvent.click(screen.getByLabelText('Edit mapping for seedbox'));
    await waitFor(() =>
      expect((screen.getByLabelText('Mapping local path') as HTMLInputElement).value).toBe('/mnt/dl/')
    );
    fireEvent.change(screen.getByLabelText('Mapping local path'), { target: { value: '/mnt/new/' } });
    fireEvent.click(screen.getByText('Save mapping'));

    await waitFor(() => {
      const put = fetchImpl.mock.calls.find(
        ([url, opts]) => String(url).endsWith('/api/v3/remotepathmapping/3') && opts?.method === 'PUT'
      );
      expect(put).toBeTruthy();
      const body = JSON.parse((put![1] as RequestInit).body as string);
      expect(body.localPath).toBe('/mnt/new/');
    });
  });

  it('confirms then DELETEs a mapping', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(MAPPINGS)) // load
      .mockResolvedValueOnce(new Response(null, { status: 200 })) // delete
      .mockResolvedValueOnce(jsonResponse([])); // reload
    const client = new CellarrClient({ fetchImpl });
    render(<RemotePathMappings client={client} />);
    await waitFor(() => expect(screen.getByLabelText('Remove mapping for seedbox')).toBeTruthy());

    fireEvent.click(screen.getByLabelText('Remove mapping for seedbox'));
    await waitFor(() => expect(screen.getByRole('alertdialog')).toBeTruthy());

    fireEvent.click(screen.getByRole('button', { name: 'Remove mapping' }));
    await waitFor(() => {
      const del = fetchImpl.mock.calls.find(
        ([url, opts]) => String(url).endsWith('/api/v3/remotepathmapping/3') && opts?.method === 'DELETE'
      );
      expect(del).toBeTruthy();
    });
  });
});
