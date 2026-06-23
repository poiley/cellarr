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

  it('builds query strings for quality profiles and history', async () => {
    const fetchImpl = vi.fn().mockImplementation(() => Promise.resolve(jsonResponse([])));
    const client = new CellarrClient({ fetchImpl });
    await client.getQualityProfiles(['a', 'b']);
    expect(fetchImpl.mock.calls[0][0]).toBe('/api/v1/qualityprofiles?ids=a%2Cb');
    await client.getHistory('cid');
    expect(fetchImpl.mock.calls[1][0]).toBe('/api/v1/history?content=cid');
  });
});
