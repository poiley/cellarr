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

  it('shows a success indicator when the test passes', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(INDEXERS)) // load
      .mockResolvedValueOnce(jsonResponse({ ok: true })); // test
    const client = new CellarrClient({ fetchImpl });
    render(
      <IntegrationSection kind="indexers" title="Indexers" implementations={['Prowlarr']} client={client} />
    );
    await waitFor(() => expect(screen.getAllByText('Prowlarr').length).toBeGreaterThan(0));

    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'My indexer' } });
    fireEvent.click(screen.getByText('Test'));
    await waitFor(() =>
      expect(screen.getByText(/connection successful/i)).toBeTruthy()
    );
    // Test goes through the working v3 route (the v1 /indexers/test route does
    // not exist on the daemon — it 404-falls-through to the SPA).
    const testCall = fetchImpl.mock.calls.find(([url]) =>
      String(url).endsWith('/api/v3/indexer/test')
    );
    expect(testCall).toBeTruthy();
  });

  it('shows a failure indicator when the test fails', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(INDEXERS))
      .mockResolvedValueOnce(jsonResponse({ code: 'connection_refused', message: 'host down' }, 502));
    const client = new CellarrClient({ fetchImpl });
    render(
      <IntegrationSection kind="indexers" title="Indexers" implementations={['Prowlarr']} client={client} />
    );
    await waitFor(() => expect(screen.getAllByText('Prowlarr').length).toBeGreaterThan(0));

    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'My indexer' } });
    fireEvent.click(screen.getByText('Test'));
    // The inline indicator carries the failure (the same outcome is also toasted,
    // but the toast lives in the ToastProvider which is not mounted in this unit).
    await waitFor(() => expect(screen.getByText(/host down/)).toBeTruthy());
  });

  it('confirms before deleting a config and then DELETEs it', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse([{ id: '7', name: 'Prowlarr', implementation: 'Prowlarr', enabled: true }]))
      .mockResolvedValueOnce(new Response(null, { status: 204 })) // delete
      .mockResolvedValueOnce(jsonResponse([])); // reload
    const client = new CellarrClient({ fetchImpl });
    render(
      <IntegrationSection kind="indexers" title="Indexers" implementations={['Prowlarr']} client={client} />
    );
    await waitFor(() => expect(screen.getByLabelText('Remove Prowlarr')).toBeTruthy());

    // Clicking Remove opens a confirm dialog — no DELETE yet.
    fireEvent.click(screen.getByLabelText('Remove Prowlarr'));
    expect(
      fetchImpl.mock.calls.find(([, opts]) => opts?.method === 'DELETE')
    ).toBeFalsy();
    await waitFor(() => expect(screen.getByRole('alertdialog')).toBeTruthy());

    // Confirm in the dialog fires the v3 numeric DELETE.
    fireEvent.click(screen.getByRole('button', { name: 'Remove indexer' }));
    await waitFor(() => {
      const del = fetchImpl.mock.calls.find(
        ([url, opts]) => String(url).endsWith('/api/v3/indexer/7') && opts?.method === 'DELETE'
      );
      expect(del).toBeTruthy();
    });
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
      // Save goes through the working v3 create route with a Radarr-shaped body
      // (host lives inside fields[], not as a flat property). The v1
      // /downloadclients POST the screen used to send had no working test/shape.
      const postCall = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).endsWith('/api/v3/downloadclient') && opts?.method === 'POST'
      );
      expect(postCall).toBeTruthy();
      const body = JSON.parse((postCall![1] as RequestInit).body as string);
      expect(body.name).toBe('qbit');
      const hostField = (body.fields as Array<{ name: string; value: unknown }>).find(
        (f) => f.name === 'host'
      );
      expect(hostField?.value).toBe('localhost');
    });
  });
});
