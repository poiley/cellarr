import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import Tags from '@app/settings/_components/Tags';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const TAGS = [
  { id: 1, label: 'hd' },
  { id: 2, label: 'kids' },
];

describe('Tags settings manager', () => {
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

  it('lists existing tags with their id', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse(TAGS));
    const client = new CellarrClient({ fetchImpl });
    render(<Tags client={client} />);

    await waitFor(() => expect(screen.getByText('#hd')).toBeTruthy());
    expect(screen.getByText('#kids')).toBeTruthy();
    expect(screen.getByText('id 1')).toBeTruthy();
  });

  it('POSTs a new tag to /api/v3/tag on add', async () => {
    let listCalls = 0;
    const fetchImpl = vi.fn().mockImplementation((url: string, opts?: RequestInit) => {
      const u = String(url);
      if (u.endsWith('/api/v3/tag')) {
        if (opts?.method === 'POST') return Promise.resolve(jsonResponse({ id: 5, label: 'archive' }));
        listCalls += 1;
        return Promise.resolve(jsonResponse(listCalls > 1 ? [{ id: 5, label: 'archive' }] : []));
      }
      return Promise.resolve(jsonResponse([]));
    });
    const client = new CellarrClient({ fetchImpl });
    render(<Tags client={client} />);

    await waitFor(() => expect(screen.getByText(/no tags yet/i)).toBeTruthy());
    fireEvent.change(screen.getByLabelText('Tag label'), { target: { value: 'archive' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add tag' }));

    await waitFor(() => {
      const post = fetchImpl.mock.calls.find(
        ([url, o]) => String(url).endsWith('/api/v3/tag') && o?.method === 'POST'
      );
      expect(post).toBeTruthy();
      const body = JSON.parse((post![1] as RequestInit).body as string);
      expect(body.label).toBe('archive');
    });
    // The list reloads after a successful create.
    await waitFor(() => expect(listCalls).toBeGreaterThan(1));
  });

  it('confirms before deleting then DELETEs the tag by id', async () => {
    let deleted = false;
    const fetchImpl = vi.fn().mockImplementation((url: string, opts?: RequestInit) => {
      const u = String(url);
      if (u.includes('/api/v3/tag/') && opts?.method === 'DELETE') {
        deleted = true;
        return Promise.resolve(new Response(null, { status: 200 }));
      }
      if (u.endsWith('/api/v3/tag')) return Promise.resolve(jsonResponse(deleted ? [] : TAGS));
      return Promise.resolve(jsonResponse([]));
    });
    const client = new CellarrClient({ fetchImpl });
    render(<Tags client={client} />);

    await waitFor(() => expect(screen.getByLabelText('Remove tag hd')).toBeTruthy());
    fireEvent.click(screen.getByLabelText('Remove tag hd'));
    // No DELETE before confirming.
    expect(fetchImpl.mock.calls.find(([, o]) => o?.method === 'DELETE')).toBeFalsy();
    await waitFor(() => expect(screen.getByRole('alertdialog')).toBeTruthy());

    fireEvent.click(screen.getByRole('button', { name: 'Remove tag' }));
    await waitFor(() => {
      const del = fetchImpl.mock.calls.find(
        ([url, o]) => String(url).endsWith('/api/v3/tag/1') && o?.method === 'DELETE'
      );
      expect(del).toBeTruthy();
    });
  });
});
