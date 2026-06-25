import { describe, expect, it, vi } from 'vitest';

import { ApiError, CellarrClient } from '@lib/api/client';
import type { AuthStatus } from '@lib/api/types';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const STATUS: AuthStatus = {
  method: 'forms',
  configured: true,
  enforced: true,
  username: 'admin',
};

describe('CellarrClient auth surface', () => {
  it('POSTs /login (bare path, no /api prefix) and sends cookies', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse(STATUS));
    const client = new CellarrClient({ fetchImpl });

    const res = await client.login({ username: 'admin', password: 'pw' });
    expect(res).toMatchObject({ method: 'forms', username: 'admin' });

    const [url, init] = fetchImpl.mock.calls[0];
    expect(url).toBe('/login');
    expect((init as RequestInit).method).toBe('POST');
    expect((init as RequestInit).credentials).toBe('same-origin');
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      username: 'admin',
      password: 'pw',
    });
  });

  it('surfaces a 401 on wrong credentials as an unauthorized ApiError', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValue(
        jsonResponse({ code: 'unauthorized', message: 'bad creds' }, 401)
      );
    const client = new CellarrClient({ fetchImpl });

    await expect(client.login({ username: 'a', password: 'b' })).rejects.toMatchObject({
      code: 'unauthorized',
      status: 401,
    });
    await expect(client.login({ username: 'a', password: 'b' })).rejects.toBeInstanceOf(
      ApiError
    );
  });

  it('POSTs /logout (idempotent) on the bare path', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse({ ok: true }));
    const client = new CellarrClient({ fetchImpl });

    const res = await client.logout();
    expect(res).toEqual({ ok: true });
    expect(fetchImpl.mock.calls[0][0]).toBe('/logout');
    expect((fetchImpl.mock.calls[0][1] as RequestInit).method).toBe('POST');
  });

  it('GETs /api/v1/auth/config', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse(STATUS));
    const client = new CellarrClient({ fetchImpl });

    const res = await client.getAuthConfig();
    expect(res.method).toBe('forms');
    expect(fetchImpl.mock.calls[0][0]).toBe('/api/v1/auth/config');
  });

  it('PUTs the method to /api/v1/auth/config', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValue(jsonResponse({ ...STATUS, method: 'basic' }));
    const client = new CellarrClient({ fetchImpl });

    const res = await client.setAuthMethod('basic');
    expect(res.method).toBe('basic');

    const [url, init] = fetchImpl.mock.calls[0];
    expect(url).toBe('/api/v1/auth/config');
    expect((init as RequestInit).method).toBe('PUT');
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({ method: 'basic' });
  });

  it('POSTs the credential to /api/v1/auth/credential', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValue(jsonResponse({ ...STATUS, configured: true }));
    const client = new CellarrClient({ fetchImpl });

    await client.setCredential({ username: 'admin', password: 'hunter2' });

    const [url, init] = fetchImpl.mock.calls[0];
    expect(url).toBe('/api/v1/auth/credential');
    expect((init as RequestInit).method).toBe('POST');
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      username: 'admin',
      password: 'hunter2',
    });
  });

  it('sends cookies on the gated /api/v1 reads (so the session is seen)', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse(STATUS));
    const client = new CellarrClient({ fetchImpl });
    await client.getAuthConfig();
    expect((fetchImpl.mock.calls[0][1] as RequestInit).credentials).toBe('same-origin');
  });
});
