import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import { ThemeProvider } from '@lib/ThemeProvider';
import { ToastProvider } from '@app/_lib/ToastProvider';
import QueueActions from '@app/activity/_components/QueueActions';
import type { QueueRecord } from '@lib/api/types';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const RECORD: QueueRecord = {
  id: '12345',
  title: 'The Movie 2024',
  status: 'completed',
  protocol: 'torrent',
  category: 'movies',
  contentId: 'c-1',
  grabId: 'g-1',
};

const renderWith = (client: CellarrClient, onChanged = () => {}) =>
  render(
    <ThemeProvider>
      <ToastProvider>
        <QueueActions record={RECORD} onChanged={onChanged} client={client} />
      </ToastProvider>
    </ThemeProvider>
  );

describe('QueueActions (activity)', () => {
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

  it('removes a queue item with the right query flags', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValue(
        jsonResponse({ removed: true, removedFromClient: true, blocklisted: true })
      );
    const onChanged = vi.fn();
    renderWith(new CellarrClient({ fetchImpl }), onChanged);

    fireEvent.click(screen.getByLabelText('Remove The Movie 2024'));
    await waitFor(() => expect(screen.getByRole('dialog')).toBeTruthy());

    // The SRCL Checkbox keeps its caption in a sibling div (not a label), so reach
    // each input via its `name` and toggle it directly.
    fireEvent.click(
      document.querySelector('input[name="queue-remove-client"]') as HTMLInputElement
    );
    fireEvent.click(
      document.querySelector('input[name="queue-remove-blocklist"]') as HTMLInputElement
    );
    fireEvent.click(screen.getByLabelText('Confirm remove from queue'));

    await waitFor(() => {
      const del = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).includes('/api/v3/queue/12345') &&
          (opts as RequestInit)?.method === 'DELETE'
      );
      expect(del).toBeTruthy();
      const url = String(del![0]);
      expect(url).toContain('removeFromClient=true');
      expect(url).toContain('blocklist=true');
    });
    expect(onChanged).toHaveBeenCalled();
  });

  it('defaults the remove query flags to false when left unchecked', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValue(
        jsonResponse({ removed: true, removedFromClient: false, blocklisted: false })
      );
    renderWith(new CellarrClient({ fetchImpl }));

    fireEvent.click(screen.getByLabelText('Remove The Movie 2024'));
    await waitFor(() => expect(screen.getByRole('dialog')).toBeTruthy());
    fireEvent.click(screen.getByLabelText('Confirm remove from queue'));

    await waitFor(() => {
      const del = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).includes('/api/v3/queue/12345') &&
          (opts as RequestInit)?.method === 'DELETE'
      );
      expect(del).toBeTruthy();
      const url = String(del![0]);
      expect(url).toContain('removeFromClient=false');
      expect(url).toContain('blocklist=false');
    });
  });

  it('changes the category via PUT', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse({ id: '12345', category: 'uhd' }));
    renderWith(new CellarrClient({ fetchImpl }));

    fireEvent.click(screen.getByLabelText('Change category for The Movie 2024'));
    await waitFor(() => expect(screen.getByLabelText('Queue category')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Queue category'), { target: { value: 'uhd' } });
    fireEvent.click(screen.getByLabelText('Save category'));

    await waitFor(() => {
      const put = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).includes('/api/v3/queue/12345') &&
          (opts as RequestInit)?.method === 'PUT'
      );
      expect(put).toBeTruthy();
      const body = JSON.parse((put![1] as RequestInit).body as string);
      expect(body.category).toBe('uhd');
    });
  });

  it('commits a manual import from the queue with id + path', async () => {
    const fetchImpl = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      const method = (init?.method ?? 'GET').toUpperCase();
      if (url.endsWith('/api/v3/movie')) return Promise.resolve(jsonResponse([]));
      if (url.endsWith('/api/v3/series')) return Promise.resolve(jsonResponse([]));
      if (url.endsWith('/api/v3/queue/grab') && method === 'POST') {
        return Promise.resolve(jsonResponse({ imported: true, files: 1, errors: [] }));
      }
      return Promise.resolve(jsonResponse({}));
    });
    renderWith(new CellarrClient({ fetchImpl }));

    fireEvent.click(screen.getByLabelText('Manual import The Movie 2024'));
    await waitFor(() => expect(screen.getByLabelText('Import path')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Import path'), {
      target: { value: '/downloads/movie.mkv' },
    });
    fireEvent.click(screen.getByLabelText('Confirm file import action'));

    await waitFor(() => {
      const grab = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).endsWith('/api/v3/queue/grab') &&
          (opts as RequestInit)?.method === 'POST'
      );
      expect(grab).toBeTruthy();
      const body = JSON.parse((grab![1] as RequestInit).body as string);
      expect(body.id).toBe('12345');
      expect(body.path).toBe('/downloads/movie.mkv');
      // The record carried a contentId, so it is sent as the default match.
      expect(body.contentId).toBe('c-1');
    });
  });
});
