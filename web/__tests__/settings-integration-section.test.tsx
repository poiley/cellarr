import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import IntegrationSection from '@app/settings/_components/IntegrationSection';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const INDEXERS = [{ id: 'prowl', name: 'Prowlarr', implementation: 'Prowlarr', enabled: true }];

describe('IntegrationSection (indexers / clients)', () => {
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

  it('lists existing configs', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse(INDEXERS));
    const client = new CellarrClient({ fetchImpl });
    render(
      <IntegrationSection kind="indexers" title="Indexers" implementations={['Prowlarr']} client={client} />
    );
    await waitFor(() => expect(screen.getAllByText('Prowlarr').length).toBeGreaterThan(0));
  });

  it('shows a success banner when the test passes', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(INDEXERS)) // load
      .mockResolvedValueOnce(jsonResponse({ ok: true })); // test
    const client = new CellarrClient({ fetchImpl });
    render(
      <IntegrationSection kind="indexers" title="Indexers" implementations={['Prowlarr']} client={client} />
    );
    await waitFor(() => expect(screen.getAllByText('Prowlarr').length).toBeGreaterThan(0));

    fireEvent.click(screen.getByText('Test'));
    await waitFor(() =>
      expect(screen.getByText(/connection successful/i)).toBeTruthy()
    );
    const testCall = fetchImpl.mock.calls.find(([url]) => String(url).endsWith('/indexers/test'));
    expect(testCall).toBeTruthy();
  });

  it('shows an error banner when the test fails', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(INDEXERS))
      .mockResolvedValueOnce(jsonResponse({ code: 'connection_refused', message: 'host down' }, 502));
    const client = new CellarrClient({ fetchImpl });
    render(
      <IntegrationSection kind="indexers" title="Indexers" implementations={['Prowlarr']} client={client} />
    );
    await waitFor(() => expect(screen.getAllByText('Prowlarr').length).toBeGreaterThan(0));

    fireEvent.click(screen.getByText('Test'));
    await waitFor(() => expect(screen.getByRole('alert').textContent).toMatch(/host down/));
  });

  it('POSTs a new config on save', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse([])) // load (empty)
      .mockResolvedValueOnce(jsonResponse({ id: 'new' })) // save
      .mockResolvedValueOnce(jsonResponse([])); // reload
    const client = new CellarrClient({ fetchImpl });
    render(
      <IntegrationSection
        kind="downloadclients"
        title="Download Clients"
        implementations={['qBittorrent']}
        client={client}
      />
    );
    await waitFor(() => expect(screen.getByText(/no download clients/i)).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'qbit' } });
    fireEvent.change(screen.getByLabelText('Host'), { target: { value: 'localhost' } });
    fireEvent.click(screen.getByText('Save'));

    await waitFor(() => {
      const postCall = fetchImpl.mock.calls.find(
        ([url, opts]) => String(url).endsWith('/downloadclients') && opts?.method === 'POST'
      );
      expect(postCall).toBeTruthy();
      const body = JSON.parse((postCall![1] as RequestInit).body as string);
      expect(body.name).toBe('qbit');
      expect(body.host).toBe('localhost');
    });
  });
});
