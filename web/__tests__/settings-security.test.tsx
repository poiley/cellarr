import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import * as React from 'react';

import { CellarrClient } from '@lib/api/client';
import { ToastProvider } from '@app/_lib/ToastProvider';
import Security from '@app/settings/_components/Security';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

// Route auth fetches by URL+method so the test is order-independent. The setters
// echo a refreshed AuthStatus; the GET seeds the form.
function routedFetch(config: Record<string, unknown>) {
  return vi.fn().mockImplementation((url: string, opts?: RequestInit) => {
    const u = String(url);
    const method = (opts?.method ?? 'GET').toUpperCase();
    if (u.endsWith('/api/v1/auth/config') && method === 'GET') {
      return Promise.resolve(jsonResponse(config));
    }
    if (u.endsWith('/api/v1/auth/config') && method === 'PUT') {
      const body = JSON.parse((opts?.body as string) ?? '{}');
      return Promise.resolve(jsonResponse({ ...config, method: body.method }));
    }
    if (u.endsWith('/api/v1/auth/credential') && method === 'POST') {
      const body = JSON.parse((opts?.body as string) ?? '{}');
      return Promise.resolve(
        jsonResponse({ ...config, configured: true, username: body.username })
      );
    }
    return Promise.resolve(jsonResponse({}));
  });
}

function renderSecurity(fetchImpl: ReturnType<typeof routedFetch>) {
  const client = new CellarrClient({ fetchImpl: fetchImpl as unknown as typeof fetch });
  render(
    <ToastProvider>
      <Security client={client} />
    </ToastProvider>
  );
  return client;
}

describe('Security (settings)', () => {
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

  it('loads the config and seeds the username from it', async () => {
    const fetchImpl = routedFetch({
      method: 'forms',
      configured: true,
      enforced: true,
      username: 'admin',
    });
    renderSecurity(fetchImpl);

    await waitFor(() =>
      expect((screen.getByLabelText('Admin username') as HTMLInputElement).value).toBe(
        'admin'
      )
    );
  });

  it('PUTs the chosen method to /api/v1/auth/config on save', async () => {
    const fetchImpl = routedFetch({
      method: 'forms',
      configured: true,
      enforced: true,
      username: 'admin',
    });
    renderSecurity(fetchImpl);

    await waitFor(() => expect(screen.getByText('Save method')).toBeTruthy());

    // Open the SRCL Select and pick "Basic".
    fireEvent.click(screen.getByText('Forms (Login Page)'));
    fireEvent.click(screen.getByText('Basic (Browser Popup)'));
    fireEvent.click(screen.getByText('Save method'));

    await waitFor(() => {
      const put = fetchImpl.mock.calls.find(
        ([u, o]) =>
          String(u).endsWith('/api/v1/auth/config') &&
          (o as RequestInit)?.method === 'PUT'
      );
      expect(put).toBeTruthy();
      expect(JSON.parse((put![1] as RequestInit).body as string)).toEqual({
        method: 'basic',
      });
    });
  });

  it('blocks enabling a method when no credential is configured yet', async () => {
    const fetchImpl = routedFetch({
      method: 'none',
      configured: false,
      enforced: false,
    });
    renderSecurity(fetchImpl);

    await waitFor(() => expect(screen.getByText('Save method')).toBeTruthy());

    fireEvent.click(screen.getByText('None'));
    fireEvent.click(screen.getByText('Forms (Login Page)'));
    fireEvent.click(screen.getByText('Save method'));

    await waitFor(() =>
      expect(
        screen.getByText(/Set an admin username and password before enabling/)
      ).toBeTruthy()
    );
    const put = fetchImpl.mock.calls.find(
      ([u, o]) =>
        String(u).endsWith('/api/v1/auth/config') &&
        (o as RequestInit)?.method === 'PUT'
    );
    expect(put).toBeUndefined();
  });

  it('POSTs the admin username + password to /api/v1/auth/credential on save', async () => {
    const fetchImpl = routedFetch({
      method: 'forms',
      configured: false,
      enforced: false,
    });
    renderSecurity(fetchImpl);

    await waitFor(() => expect(screen.getByLabelText('Admin username')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Admin username'), {
      target: { value: 'root' },
    });
    fireEvent.change(screen.getByLabelText('Admin password'), {
      target: { value: 'sekret-12345' },
    });
    fireEvent.change(screen.getByLabelText('Confirm admin password'), {
      target: { value: 'sekret-12345' },
    });
    fireEvent.click(screen.getByText('Save credentials'));

    await waitFor(() => {
      const post = fetchImpl.mock.calls.find(
        ([u, o]) =>
          String(u).endsWith('/api/v1/auth/credential') &&
          (o as RequestInit)?.method === 'POST'
      );
      expect(post).toBeTruthy();
      expect(JSON.parse((post![1] as RequestInit).body as string)).toEqual({
        username: 'root',
        password: 'sekret-12345',
      });
    });
  });

  it('refuses to POST when password and confirmation differ', async () => {
    const fetchImpl = routedFetch({
      method: 'none',
      configured: false,
      enforced: false,
    });
    renderSecurity(fetchImpl);

    await waitFor(() => expect(screen.getByLabelText('Admin username')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Admin username'), {
      target: { value: 'root' },
    });
    fireEvent.change(screen.getByLabelText('Admin password'), {
      target: { value: 'aaa' },
    });
    fireEvent.change(screen.getByLabelText('Confirm admin password'), {
      target: { value: 'bbb' },
    });
    fireEvent.click(screen.getByText('Save credentials'));

    await waitFor(() =>
      expect(screen.getByText(/do not match/)).toBeTruthy()
    );
    const post = fetchImpl.mock.calls.find(
      ([u, o]) =>
        String(u).endsWith('/api/v1/auth/credential') &&
        (o as RequestInit)?.method === 'POST'
    );
    expect(post).toBeUndefined();
  });

  it('toggles the admin password between hidden and shown', async () => {
    const fetchImpl = routedFetch({ method: 'none', configured: false, enforced: false });
    renderSecurity(fetchImpl);

    await waitFor(() => expect(screen.getByLabelText('Admin password')).toBeTruthy());

    const password = screen.getByLabelText('Admin password') as HTMLInputElement;
    // Hidden by default.
    expect(password.type).toBe('password');

    // Reveal: the toggle (labelled "Show password") flips the input to text.
    fireEvent.click(screen.getByLabelText('Show password'));
    expect((screen.getByLabelText('Admin password') as HTMLInputElement).type).toBe('text');

    // Hide again.
    fireEvent.click(screen.getByLabelText('Hide password'));
    expect((screen.getByLabelText('Admin password') as HTMLInputElement).type).toBe('password');
  });
});
