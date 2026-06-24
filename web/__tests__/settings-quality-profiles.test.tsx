import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import QualityProfiles from '@app/settings/_components/QualityProfiles';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

function findSaveCall(fetchImpl: ReturnType<typeof vi.fn>) {
  // Edits route to PUT /api/v3/qualityprofile/:id.
  return fetchImpl.mock.calls.find(
    ([url, opts]) => String(url).includes('/qualityprofile/hd') && opts?.method === 'PUT'
  );
}

function findCreateCall(fetchImpl: ReturnType<typeof vi.fn>) {
  // Creates route to POST /api/v3/qualityprofile (no id segment).
  return fetchImpl.mock.calls.find(
    ([url, opts]) =>
      /\/qualityprofile($|\?)/.test(String(url)) && opts?.method === 'POST'
  );
}

function findDeleteCall(fetchImpl: ReturnType<typeof vi.fn>) {
  return fetchImpl.mock.calls.find(
    ([url, opts]) => String(url).includes('/qualityprofile/hd') && opts?.method === 'DELETE'
  );
}

// The real Radarr-compatible shape served by GET /api/v3/qualityprofile.
const PROFILE = {
  id: 'hd',
  name: 'HD-1080p',
  upgradeAllowed: true,
  cutoff: 21,
  cutoffFormatScore: 100,
  minFormatScore: 50,
  minUpgradeFormatScore: 1,
  language: { id: -2, name: 'Original' },
  formatItems: [],
  // The profile carries the unhelpful "rank-N" placeholder names the seeded
  // daemon serves; the editor must resolve these to human names via the
  // quality definitions (id 20 -> WEBDL-1080p, id 21 -> Bluray-1080p).
  items: [
    { allowed: true, items: [], quality: { id: 20, name: 'rank-20', resolution: 0, source: 'unknown' } },
    { allowed: true, items: [], quality: { id: 21, name: 'rank-21', resolution: 0, source: 'unknown' } },
  ],
};

const DEFINITIONS = [
  { id: 1, title: 'WEBDL-1080p', weight: 1, minSize: 0, maxSize: null, preferredSize: null, quality: { id: 20, name: 'WEBDL-1080p', resolution: 0, source: 'unknown' } },
  { id: 2, title: 'Bluray-1080p', weight: 2, minSize: 0, maxSize: null, preferredSize: null, quality: { id: 21, name: 'Bluray-1080p', resolution: 0, source: 'unknown' } },
];

// Route a request to the right canned response by URL/method, so the order in
// which the profiles + definitions loaders fire doesn't matter.
function makeRouter(profiles: unknown = [PROFILE]) {
  return vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const method = init?.method ?? 'GET';
    if (url.includes('/qualitydefinition')) return Promise.resolve(jsonResponse(DEFINITIONS));
    if (url.includes('/qualityprofile') && method === 'GET') {
      return Promise.resolve(jsonResponse(profiles));
    }
    if (url.includes('/qualityprofile') && method === 'PUT') {
      return Promise.resolve(jsonResponse(PROFILE));
    }
    if (url.includes('/qualityprofile') && method === 'POST') {
      return Promise.resolve(jsonResponse({ ...PROFILE, id: 'new1', name: 'Created' }));
    }
    if (url.includes('/qualityprofile') && method === 'DELETE') {
      return Promise.resolve(new Response(null, { status: 204 }));
    }
    return Promise.resolve(jsonResponse([]));
  });
}

describe('QualityProfiles (settings)', () => {
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

  it('shows a loading state then renders the loaded profile', async () => {
    const fetchImpl = makeRouter();
    const client = new CellarrClient({ fetchImpl });
    render(<QualityProfiles client={client} />);
    expect(screen.getByRole('status').textContent).toMatch(/loading/i);
    await waitFor(() => expect(screen.getByLabelText('Profile name')).toBeTruthy());
    expect((screen.getByLabelText('Profile name') as HTMLInputElement).value).toBe('HD-1080p');
    expect(screen.getByLabelText('Move WEBDL-1080p down')).toBeTruthy();
    // The "rank-N" placeholder carried on the profile is resolved to the human
    // name from the quality definitions — it must not leak into the UI.
    expect(screen.queryByLabelText(/Move rank-20/)).toBeNull();
    // Bluray-1080p appears in both the qualities list and the cutoff selector.
    expect(screen.getAllByText('Bluray-1080p').length).toBeGreaterThan(0);
  });

  it('renders an error banner when the load fails', async () => {
    const fetchImpl = vi.fn((input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes('/qualitydefinition')) return Promise.resolve(jsonResponse(DEFINITIONS));
      return Promise.resolve(jsonResponse({ code: 'unauthorized', message: 'no api key' }, 401));
    });
    const client = new CellarrClient({ fetchImpl });
    render(<QualityProfiles client={client} />);
    await waitFor(() => expect(screen.getByRole('alert').textContent).toMatch(/no api key/));
  });

  it('renders an empty state with a create affordance when there are no profiles', async () => {
    const fetchImpl = makeRouter([]);
    const client = new CellarrClient({ fetchImpl });
    render(<QualityProfiles client={client} />);
    await waitFor(() =>
      expect(screen.getByRole('status').textContent).toMatch(/no quality profiles/i)
    );
    expect(screen.getByText('New profile')).toBeTruthy();
  });

  it('PUTs the edited profile on save', async () => {
    const fetchImpl = makeRouter();
    const client = new CellarrClient({ fetchImpl });
    render(<QualityProfiles client={client} />);
    await waitFor(() => expect(screen.getByLabelText('Profile name')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Profile name'), { target: { value: 'My HD' } });
    fireEvent.click(screen.getByText('Save profile'));

    await waitFor(() => expect(findSaveCall(fetchImpl)).toBeTruthy());
    const body = JSON.parse((findSaveCall(fetchImpl)![1] as RequestInit).body as string);
    expect(body.name).toBe('My HD');
    expect(body.items).toHaveLength(2);
  });

  it('reorders qualities with the move controls', async () => {
    const fetchImpl = makeRouter();
    const client = new CellarrClient({ fetchImpl });
    render(<QualityProfiles client={client} />);
    await waitFor(() => expect(screen.getByLabelText('Move WEBDL-1080p down')).toBeTruthy());

    fireEvent.click(screen.getByLabelText('Move WEBDL-1080p down'));
    fireEvent.click(screen.getByText('Save profile'));

    await waitFor(() => expect(findSaveCall(fetchImpl)).toBeTruthy());
    const body = JSON.parse((findSaveCall(fetchImpl)![1] as RequestInit).body as string);
    expect(body.items[0].quality.id).toBe(21); // Bluray-1080p moved up
    expect(body.items[1].quality.id).toBe(20); // WEBDL-1080p moved down
  });

  it('creates a new profile via POST, seeding qualities from definitions', async () => {
    const fetchImpl = makeRouter();
    const client = new CellarrClient({ fetchImpl });
    render(<QualityProfiles client={client} />);
    await waitFor(() => expect(screen.getByLabelText('Profile name')).toBeTruthy());

    fireEvent.click(screen.getByText('New profile'));
    // Now in create mode: name is blank, the ladder is seeded from definitions.
    await waitFor(() =>
      expect((screen.getByLabelText('Profile name') as HTMLInputElement).value).toBe('')
    );
    expect(screen.getByText('Create profile')).toBeTruthy();
    // The ladder is seeded from the quality definitions.
    await waitFor(() => expect(screen.getByLabelText('Move WEBDL-1080p down')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Profile name'), { target: { value: 'Fresh' } });
    fireEvent.click(screen.getByText('Create profile'));

    await waitFor(() => expect(findCreateCall(fetchImpl)).toBeTruthy());
    const body = JSON.parse((findCreateCall(fetchImpl)![1] as RequestInit).body as string);
    expect(body.name).toBe('Fresh');
    expect(body.id).toBeUndefined();
    expect(body.items.length).toBe(2);
  });

  it('deletes a profile after a confirm click', async () => {
    const fetchImpl = makeRouter();
    const client = new CellarrClient({ fetchImpl });
    render(<QualityProfiles client={client} />);
    await waitFor(() => expect(screen.getByText('Delete profile')).toBeTruthy());

    // First click only arms the confirm — no DELETE yet.
    fireEvent.click(screen.getByText('Delete profile'));
    expect(findDeleteCall(fetchImpl)).toBeFalsy();
    await waitFor(() => expect(screen.getByText('Confirm delete')).toBeTruthy());

    fireEvent.click(screen.getByText('Confirm delete'));
    await waitFor(() => expect(findDeleteCall(fetchImpl)).toBeTruthy());
  });
});
