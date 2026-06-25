import { describe, expect, it, vi } from 'vitest';

import { ApiError, CellarrClient, resolveBaseUrl } from '@lib/api/client';

function jsonResponse(body: unknown, init: { status?: number } = {}) {
  return new Response(JSON.stringify(body), {
    status: init.status ?? 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

describe('CellarrClient', () => {
  it('defaults to a same-origin base (empty)', () => {
    expect(resolveBaseUrl()).toBe('');
  });

  it('GETs system status against /api/v1 same-origin', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(
      jsonResponse({
        app_name: 'cellarr',
        version: '0.1.0',
        auth_enabled: false,
        library_count: 2,
        indexer_count: 1,
        download_client_count: 0,
      })
    );
    const client = new CellarrClient({ fetchImpl });
    const status = await client.systemStatus();
    expect(status.app_name).toBe('cellarr');
    const url = fetchImpl.mock.calls[0][0] as string;
    expect(url).toBe('/api/v1/system/status');
  });

  it('honors an explicit base URL override', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse([]));
    const client = new CellarrClient({ baseUrl: 'http://localhost:9999/', fetchImpl });
    await client.listLibraries();
    expect(fetchImpl.mock.calls[0][0]).toBe('http://localhost:9999/api/v1/libraries');
  });

  it('surfaces the structured {code,message} error body as ApiError', async () => {
    const fetchImpl = vi
      .fn()
      .mockImplementation(() =>
        Promise.resolve(
          jsonResponse({ code: 'not_found', message: 'library x not found' }, { status: 404 })
        )
      );
    const client = new CellarrClient({ fetchImpl });
    await expect(client.getLibrary('x')).rejects.toMatchObject({
      code: 'not_found',
      message: 'library x not found',
      status: 404,
    });
    await expect(client.getLibrary('x')).rejects.toBeInstanceOf(ApiError);
  });

  it('maps network failures to a network_error ApiError', async () => {
    const fetchImpl = vi.fn().mockRejectedValue(new Error('connection refused'));
    const client = new CellarrClient({ fetchImpl });
    await expect(client.systemStatus()).rejects.toMatchObject({ code: 'network_error', status: 0 });
  });

  it('sends the API key header when configured', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse({ job_id: 'j', name: 'RssSync', status: 'accepted' }));
    const client = new CellarrClient({ apiKey: 'secret', fetchImpl });
    await client.runCommand('RssSync');
    const init = fetchImpl.mock.calls[0][1] as RequestInit;
    expect((init.headers as Record<string, string>)['X-Api-Key']).toBe('secret');
    expect(init.method).toBe('POST');
  });

  it('reads quality profiles from the v3 shim (where the data lives)', async () => {
    // The seeded profiles live at /api/v3/qualityprofile; the native
    // /api/v1/qualityprofiles route returns [] — this is the profiles read bug.
    const fetchImpl = vi.fn().mockImplementation(() => Promise.resolve(jsonResponse([])));
    const client = new CellarrClient({ fetchImpl });
    await client.getQualityProfiles();
    expect(fetchImpl.mock.calls[0][0]).toBe('/api/v3/qualityprofile');
  });

  it('builds query strings for history', async () => {
    const fetchImpl = vi.fn().mockImplementation(() => Promise.resolve(jsonResponse([])));
    const client = new CellarrClient({ fetchImpl });
    await client.getHistory('cid');
    expect(fetchImpl.mock.calls[0][0]).toBe('/api/v1/history?content=cid');
  });

  it('targets the v3 shim via requestV3 and the typed v3 helpers', async () => {
    const fetchImpl = vi.fn().mockImplementation(() => Promise.resolve(jsonResponse([])));
    const client = new CellarrClient({ fetchImpl });
    await client.listMovies();
    expect(fetchImpl.mock.calls[0][0]).toBe('/api/v3/movie');
    await client.listEpisodes('s1');
    expect(fetchImpl.mock.calls[1][0]).toBe('/api/v3/episode?seriesId=s1');
  });

  it('lists, creates, deletes and restores backups on /api/v3', async () => {
    const fetchImpl = vi.fn().mockImplementation(() => Promise.resolve(jsonResponse([])));
    const client = new CellarrClient({ fetchImpl });

    await client.listBackups();
    expect(fetchImpl.mock.calls[0][0]).toBe('/api/v3/system/backup');

    await client.createBackup();
    expect(fetchImpl.mock.calls[1][0]).toBe('/api/v3/system/backup');
    expect((fetchImpl.mock.calls[1][1] as RequestInit).method).toBe('POST');

    await client.deleteBackup('bk-1');
    expect(fetchImpl.mock.calls[2][0]).toBe('/api/v3/system/backup/bk-1');
    expect((fetchImpl.mock.calls[2][1] as RequestInit).method).toBe('DELETE');

    await client.restoreBackup('bk-1');
    expect(fetchImpl.mock.calls[3][0]).toBe('/api/v3/system/backup/restore/bk-1');
    expect((fetchImpl.mock.calls[3][1] as RequestInit).method).toBe('POST');
  });

  it('builds a same-origin backup download URL', () => {
    const client = new CellarrClient({ baseUrl: 'http://host:1/' });
    expect(client.backupDownloadUrl('bk-9')).toBe('http://host:1/api/v3/system/backup/bk-9');
  });

  it('lists log files and fetches a tail as plain text with a line bound', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse([{ id: 1, filename: 'cellarr.txt' }]))
      .mockResolvedValueOnce(new Response('line1\nline2', { status: 200 }));
    const client = new CellarrClient({ fetchImpl });

    const files = await client.listLogFiles();
    expect(fetchImpl.mock.calls[0][0]).toBe('/api/v3/log/file');
    expect(files[0].filename).toBe('cellarr.txt');

    const text = await client.getLogFile('cellarr.txt', 250);
    expect(fetchImpl.mock.calls[1][0]).toBe('/api/v3/log/file/cellarr.txt?lines=250');
    expect(text).toBe('line1\nline2');
  });

  it('surfaces a structured error from the log tail route', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValue(jsonResponse({ code: 'not_found', message: 'no such log' }, { status: 404 }));
    const client = new CellarrClient({ fetchImpl });
    await expect(client.getLogFile('../etc/passwd')).rejects.toMatchObject({
      code: 'not_found',
    });
  });
});
