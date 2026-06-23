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
  return fetchImpl.mock.calls.find(
    ([url, opts]) => String(url).includes('/qualityprofiles/hd') && opts?.method === 'POST'
  );
}

const PROFILE = {
  id: 'hd',
  name: 'HD-1080p',
  upgrades_allowed: true,
  cutoff: 'bluray',
  min_format_score: 50,
  qualities: [
    { id: 'web', name: 'WEB-1080p', allowed: true },
    { id: 'bluray', name: 'Bluray-1080p', allowed: true },
  ],
};

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
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse([PROFILE]));
    const client = new CellarrClient({ fetchImpl });
    render(<QualityProfiles client={client} />);
    expect(screen.getByRole('status').textContent).toMatch(/loading/i);
    await waitFor(() => expect(screen.getByLabelText('Profile name')).toBeTruthy());
    expect((screen.getByLabelText('Profile name') as HTMLInputElement).value).toBe('HD-1080p');
    expect(screen.getByLabelText('Move WEB-1080p down')).toBeTruthy();
  });

  it('renders an error banner when the load fails', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValue(jsonResponse({ code: 'unauthorized', message: 'no api key' }, 401));
    const client = new CellarrClient({ fetchImpl });
    render(<QualityProfiles client={client} />);
    await waitFor(() => expect(screen.getByRole('alert').textContent).toMatch(/no api key/));
  });

  it('renders an empty state when there are no profiles', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse([]));
    const client = new CellarrClient({ fetchImpl });
    render(<QualityProfiles client={client} />);
    await waitFor(() =>
      expect(screen.getByRole('status').textContent).toMatch(/no quality profiles/i)
    );
  });

  it('POSTs the edited profile on save', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse([PROFILE]))
      .mockResolvedValueOnce(jsonResponse(PROFILE))
      .mockResolvedValueOnce(jsonResponse([PROFILE]));
    const client = new CellarrClient({ fetchImpl });
    render(<QualityProfiles client={client} />);
    await waitFor(() => expect(screen.getByLabelText('Profile name')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Profile name'), { target: { value: 'My HD' } });
    fireEvent.click(screen.getByText('Save profile'));

    await waitFor(() => expect(findSaveCall(fetchImpl)).toBeTruthy());
    const body = JSON.parse((findSaveCall(fetchImpl)![1] as RequestInit).body as string);
    expect(body.name).toBe('My HD');
    expect(body.qualities).toHaveLength(2);
  });

  it('reorders qualities with the move controls', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse([PROFILE]))
      .mockResolvedValueOnce(jsonResponse(PROFILE))
      .mockResolvedValueOnce(jsonResponse([PROFILE]));
    const client = new CellarrClient({ fetchImpl });
    render(<QualityProfiles client={client} />);
    await waitFor(() => expect(screen.getByLabelText('Move WEB-1080p down')).toBeTruthy());

    fireEvent.click(screen.getByLabelText('Move WEB-1080p down'));
    fireEvent.click(screen.getByText('Save profile'));

    await waitFor(() => expect(findSaveCall(fetchImpl)).toBeTruthy());
    const body = JSON.parse((findSaveCall(fetchImpl)![1] as RequestInit).body as string);
    expect(body.qualities[0].id).toBe('bluray');
    expect(body.qualities[1].id).toBe('web');
  });
});
