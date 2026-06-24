import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import DelayProfiles from '@app/settings/_components/DelayProfiles';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const PROFILES = [
  {
    id: 1,
    enableUsenet: true,
    enableTorrent: true,
    preferredProtocol: 'usenet',
    usenetDelay: 0,
    torrentDelay: 30,
    bypassIfHighestQuality: true,
    tags: [],
    order: 1,
  },
];

function routedFetch(extra?: (url: string, opts?: RequestInit) => Response | undefined) {
  return vi.fn((url: string, opts?: RequestInit) => {
    const u = String(url);
    const method = opts?.method ?? 'GET';
    const fromExtra = extra?.(u, opts);
    if (fromExtra) return Promise.resolve(fromExtra);
    if (u.endsWith('/api/v3/delayprofile') && method === 'GET')
      return Promise.resolve(jsonResponse(PROFILES));
    return Promise.resolve(jsonResponse({}));
  });
}

describe('DelayProfiles (settings)', () => {
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

  it('lists profiles with their protocol + delays', async () => {
    const client = new CellarrClient({ fetchImpl: routedFetch() as never });
    render(<DelayProfiles client={client} />);
    await waitFor(() => expect(screen.getByText('#1')).toBeTruthy());
    expect(screen.getByText(/Prefer Usenet/)).toBeTruthy();
    expect(screen.getByText(/torrent 30m/)).toBeTruthy();
    expect(screen.getByLabelText('Edit delay profile 1')).toBeTruthy();
    expect(screen.getByLabelText('Delete delay profile 1')).toBeTruthy();
  });

  it('creates a new delay profile via POST', async () => {
    const fetchImpl = routedFetch((u, opts) => {
      if (u.endsWith('/api/v3/delayprofile') && (opts?.method ?? 'GET') === 'POST') {
        return jsonResponse({ id: 2, order: 2 });
      }
      return undefined;
    });
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<DelayProfiles client={client} />);
    await waitFor(() => expect(screen.getByText('#1')).toBeTruthy());

    fireEvent.click(screen.getByText('Add delay profile'));
    await waitFor(() => expect(screen.getByLabelText('Torrent delay minutes')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Torrent delay minutes'), { target: { value: '45' } });
    fireEvent.change(screen.getByLabelText('Usenet delay minutes'), { target: { value: '10' } });

    fireEvent.click(screen.getByText('Create profile'));

    await waitFor(() => {
      const post = fetchImpl.mock.calls.find(
        ([url, o]: [string, RequestInit | undefined]) =>
          String(url).endsWith('/api/v3/delayprofile') && o?.method === 'POST'
      );
      expect(post).toBeTruthy();
      const body = JSON.parse((post![1] as RequestInit).body as string);
      expect(body.torrentDelay).toBe(45);
      expect(body.usenetDelay).toBe(10);
      expect(body.preferredProtocol).toBe('either');
      expect(body.enabled).toBe(true);
    });
  });

  it('edits an existing profile via PUT', async () => {
    const fetchImpl = routedFetch((u, opts) => {
      if (/\/api\/v3\/delayprofile\/1$/.test(u) && opts?.method === 'PUT') {
        return jsonResponse({ id: 1, order: 1 });
      }
      return undefined;
    });
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<DelayProfiles client={client} />);
    await waitFor(() => expect(screen.getByText('#1')).toBeTruthy());

    fireEvent.click(screen.getByLabelText('Edit delay profile 1'));
    await waitFor(() => expect(screen.getByLabelText('Torrent delay minutes')).toBeTruthy());
    // The form is seeded from the existing profile.
    expect((screen.getByLabelText('Torrent delay minutes') as HTMLInputElement).value).toBe('30');

    fireEvent.change(screen.getByLabelText('Torrent delay minutes'), { target: { value: '60' } });
    fireEvent.click(screen.getByText('Save profile'));

    await waitFor(() => {
      const put = fetchImpl.mock.calls.find(
        ([url, o]: [string, RequestInit | undefined]) =>
          /\/api\/v3\/delayprofile\/1$/.test(String(url)) && o?.method === 'PUT'
      );
      expect(put).toBeTruthy();
      const body = JSON.parse((put![1] as RequestInit).body as string);
      expect(body.torrentDelay).toBe(60);
    });
  });

  it('deletes a profile through the confirm dialog', async () => {
    const fetchImpl = routedFetch((u, opts) => {
      if (/\/api\/v3\/delayprofile\/1$/.test(u) && opts?.method === 'DELETE') {
        return jsonResponse({});
      }
      return undefined;
    });
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<DelayProfiles client={client} />);
    await waitFor(() => expect(screen.getByText('#1')).toBeTruthy());

    fireEvent.click(screen.getByLabelText('Delete delay profile 1'));
    await waitFor(() => expect(screen.getByRole('alertdialog')).toBeTruthy());
    const dialog = screen.getByRole('alertdialog');
    fireEvent.click(within(dialog).getByLabelText('Delete delay profile'));

    await waitFor(() => {
      const del = fetchImpl.mock.calls.find(
        ([url, o]: [string, RequestInit | undefined]) =>
          /\/api\/v3\/delayprofile\/1$/.test(String(url)) && o?.method === 'DELETE'
      );
      expect(del).toBeTruthy();
    });
  });

  it('shows an empty state with no profiles', async () => {
    const fetchImpl = vi.fn(() => Promise.resolve(jsonResponse([])));
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<DelayProfiles client={client} />);
    await waitFor(() => expect(screen.getByText(/no delay profiles yet/i)).toBeTruthy());
  });
});
