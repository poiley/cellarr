import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import ReleaseProfiles from '@app/settings/_components/ReleaseProfiles';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const PROFILES = [
  {
    id: 1,
    name: 'Prefer Remux',
    enabled: true,
    required: ['1080p'],
    ignored: ['cam'],
    preferred: [{ key: 'remux', value: 50 }],
    includePreferredWhenRenaming: false,
    indexerId: 0,
    tags: [],
  },
];

function routedFetch(extra?: (url: string, opts?: RequestInit) => Response | undefined) {
  return vi.fn((url: string, opts?: RequestInit) => {
    const u = String(url);
    const method = opts?.method ?? 'GET';
    const fromExtra = extra?.(u, opts);
    if (fromExtra) return Promise.resolve(fromExtra);
    if (u.endsWith('/api/v3/releaseprofile') && method === 'GET')
      return Promise.resolve(jsonResponse(PROFILES));
    if (u.endsWith('/api/v3/tag') && method === 'GET') return Promise.resolve(jsonResponse([]));
    return Promise.resolve(jsonResponse({}));
  });
}

describe('ReleaseProfiles (settings)', () => {
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

  it('lists profiles with their enabled state and term counts', async () => {
    const client = new CellarrClient({ fetchImpl: routedFetch() as never });
    render(<ReleaseProfiles client={client} />);
    await waitFor(() => expect(screen.getByText('Prefer Remux')).toBeTruthy());
    expect(screen.getByText('enabled')).toBeTruthy();
    expect(screen.getByLabelText('Edit Prefer Remux')).toBeTruthy();
    expect(screen.getByLabelText('Delete Prefer Remux')).toBeTruthy();
  });

  it('builds a new profile and POSTs required/ignored/preferred terms', async () => {
    const fetchImpl = routedFetch((u, opts) => {
      if (u.endsWith('/api/v3/releaseprofile') && (opts?.method ?? 'GET') === 'POST') {
        return jsonResponse({ id: 2 });
      }
      return undefined;
    });
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<ReleaseProfiles client={client} />);
    await waitFor(() => expect(screen.getByText('Prefer Remux')).toBeTruthy());

    fireEvent.click(screen.getByText('Add release profile'));
    await waitFor(() => expect(screen.getByLabelText('Release profile name')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Release profile name'), {
      target: { value: 'My Profile' },
    });
    // Required term (first row pre-rendered).
    fireEvent.change(screen.getByLabelText('Required terms 1'), { target: { value: '2160p' } });
    // Ignored term.
    fireEvent.change(screen.getByLabelText('Ignored terms 1'), { target: { value: 'x264' } });
    // Preferred term + score.
    fireEvent.change(screen.getByLabelText('Preferred term 1'), { target: { value: 'remux' } });
    fireEvent.change(screen.getByLabelText('Preferred term 1 score'), { target: { value: '100' } });

    fireEvent.click(screen.getByText('Create profile'));

    await waitFor(() => {
      const post = fetchImpl.mock.calls.find(
        ([url, o]: [string, RequestInit | undefined]) =>
          String(url).endsWith('/api/v3/releaseprofile') && o?.method === 'POST'
      );
      expect(post).toBeTruthy();
      const body = JSON.parse((post![1] as RequestInit).body as string);
      expect(body.name).toBe('My Profile');
      expect(body.enabled).toBe(true);
      expect(body.required).toEqual(['2160p']);
      expect(body.ignored).toEqual(['x264']);
      expect(body.preferred).toEqual([{ key: 'remux', value: 100 }]);
      expect(body.tags).toEqual([]);
    });
  });

  it('seeds the editor from an existing profile and PUTs changes', async () => {
    const fetchImpl = routedFetch((u, opts) => {
      if (/\/api\/v3\/releaseprofile\/1$/.test(u) && opts?.method === 'PUT') {
        return jsonResponse({ id: 1 });
      }
      return undefined;
    });
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<ReleaseProfiles client={client} />);
    await waitFor(() => expect(screen.getByText('Prefer Remux')).toBeTruthy());

    fireEvent.click(screen.getByLabelText('Edit Prefer Remux'));
    await waitFor(() => expect(screen.getByLabelText('Release profile name')).toBeTruthy());
    expect((screen.getByLabelText('Required terms 1') as HTMLInputElement).value).toBe('1080p');
    expect((screen.getByLabelText('Preferred term 1 score') as HTMLInputElement).value).toBe('50');

    fireEvent.change(screen.getByLabelText('Preferred term 1 score'), { target: { value: '75' } });
    fireEvent.click(screen.getByText('Save profile'));

    await waitFor(() => {
      const put = fetchImpl.mock.calls.find(
        ([url, o]: [string, RequestInit | undefined]) =>
          /\/api\/v3\/releaseprofile\/1$/.test(String(url)) && o?.method === 'PUT'
      );
      expect(put).toBeTruthy();
      const body = JSON.parse((put![1] as RequestInit).body as string);
      expect(body.preferred).toEqual([{ key: 'remux', value: 75 }]);
    });
  });

  it('deletes a profile through the confirm dialog', async () => {
    const fetchImpl = routedFetch((u, opts) => {
      if (/\/api\/v3\/releaseprofile\/1$/.test(u) && opts?.method === 'DELETE') {
        return jsonResponse({});
      }
      return undefined;
    });
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<ReleaseProfiles client={client} />);
    await waitFor(() => expect(screen.getByText('Prefer Remux')).toBeTruthy());

    fireEvent.click(screen.getByLabelText('Delete Prefer Remux'));
    await waitFor(() => expect(screen.getByRole('alertdialog')).toBeTruthy());
    const dialog = screen.getByRole('alertdialog');
    fireEvent.click(within(dialog).getByLabelText('Delete release profile'));

    await waitFor(() => {
      const del = fetchImpl.mock.calls.find(
        ([url, o]: [string, RequestInit | undefined]) =>
          /\/api\/v3\/releaseprofile\/1$/.test(String(url)) && o?.method === 'DELETE'
      );
      expect(del).toBeTruthy();
    });
  });

  it('shows an empty state with no profiles', async () => {
    const fetchImpl = vi.fn((url: string) =>
      Promise.resolve(jsonResponse(String(url).endsWith('/api/v3/tag') ? [] : []))
    );
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<ReleaseProfiles client={client} />);
    await waitFor(() => expect(screen.getByText(/no release profiles yet/i)).toBeTruthy());
  });
});
