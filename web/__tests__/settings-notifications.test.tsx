import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import Notifications from '@app/settings/_components/Notifications';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

// The schema the daemon advertises (trimmed to the providers under test). The
// component drives its type dropdown + per-type fields entirely from this.
const SCHEMA = [
  {
    implementation: 'Discord',
    implementationName: 'Discord',
    configContract: 'DiscordSettings',
    fields: [{ order: 0, name: 'url', label: 'Webhook URL', type: 'url' }],
  },
  {
    implementation: 'Telegram',
    implementationName: 'Telegram',
    configContract: 'TelegramSettings',
    fields: [
      { order: 0, name: 'botToken', label: 'Bot Token', type: 'textbox', privacy: 'apiKey' },
      { order: 1, name: 'chatId', label: 'Chat ID', type: 'textbox' },
    ],
  },
];

const NOTIFICATIONS = [
  {
    id: 3,
    name: 'My Discord',
    implementation: 'Discord',
    implementationName: 'Discord',
    configContract: 'DiscordSettings',
    onGrab: true,
    onDownload: true,
    onUpgrade: false,
    onRename: false,
    onHealthIssue: true,
    onHealthRestored: true,
    fields: [{ order: 0, name: 'url', value: 'https://discord.example/hook' }],
    tags: [],
  },
];

// The component fires two loads on mount (list + schema). Route each fetch by URL
// so order-independence holds regardless of which effect resolves first.
function routedFetch(extra?: (url: string) => Response | undefined) {
  return vi.fn().mockImplementation((url: string) => {
    const u = String(url);
    const override = extra?.(u);
    if (override) return Promise.resolve(override);
    if (u.endsWith('/api/v3/notification/schema')) return Promise.resolve(jsonResponse(SCHEMA));
    if (u.endsWith('/api/v3/notification')) return Promise.resolve(jsonResponse(NOTIFICATIONS));
    return Promise.resolve(jsonResponse([]));
  });
}

describe('Notifications settings', () => {
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

  it('lists configured notifications with their type and events', async () => {
    const fetchImpl = routedFetch();
    const client = new CellarrClient({ fetchImpl });
    render(<Notifications client={client} />);

    await waitFor(() => expect(screen.getByText('My Discord')).toBeTruthy());
    // The list row shows the friendly type label and the subscribed-events
    // summary (onUpgrade is false on the fixture, so it is omitted).
    expect(screen.getAllByText('Discord').length).toBeGreaterThan(0);
    expect(screen.getByText('Grab, Import, Health Issue')).toBeTruthy();
  });

  it('POSTs a new notification to /api/v3/notification on save', async () => {
    // List load empty (blank form), save succeeds, reload empty.
    let listCalls = 0;
    const fetchImpl = vi.fn().mockImplementation((url: string, opts?: RequestInit) => {
      const u = String(url);
      if (u.endsWith('/api/v3/notification/schema')) return Promise.resolve(jsonResponse(SCHEMA));
      if (u.endsWith('/api/v3/notification')) {
        if (opts?.method === 'POST') return Promise.resolve(jsonResponse({ id: 9 }));
        listCalls += 1;
        return Promise.resolve(jsonResponse([]));
      }
      return Promise.resolve(jsonResponse([]));
    });

    const client = new CellarrClient({ fetchImpl });
    render(<Notifications client={client} />);

    await waitFor(() => expect(screen.getByText(/no notifications/i)).toBeTruthy());
    // Form is seeded from the schema (Discord first).
    await waitFor(() => expect(screen.getByLabelText('Name')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Alerts' } });
    fireEvent.change(screen.getByLabelText('Webhook URL'), {
      target: { value: 'https://discord.example/x' },
    });
    fireEvent.click(screen.getByText('Save'));

    await waitFor(() => {
      const post = fetchImpl.mock.calls.find(
        ([url, o]) => String(url).endsWith('/api/v3/notification') && o?.method === 'POST'
      );
      expect(post).toBeTruthy();
      const body = JSON.parse((post![1] as RequestInit).body as string);
      expect(body.name).toBe('Alerts');
      expect(body.implementation).toBe('Discord');
      const urlField = (body.fields as Array<{ name: string; value: unknown }>).find(
        (f) => f.name === 'url'
      );
      expect(urlField?.value).toBe('https://discord.example/x');
    });
    // A successful save reloads the list (initial load + post-save reload).
    await waitFor(() => expect(listCalls).toBeGreaterThan(1));
  });

  it('hits /api/v3/notification/test when Test is clicked', async () => {
    const fetchImpl = vi.fn().mockImplementation((url: string) => {
      const u = String(url);
      if (u.endsWith('/api/v3/notification/schema')) return Promise.resolve(jsonResponse(SCHEMA));
      if (u.endsWith('/api/v3/notification/test'))
        return Promise.resolve(jsonResponse({ isValid: true }));
      if (u.endsWith('/api/v3/notification')) return Promise.resolve(jsonResponse([]));
      return Promise.resolve(jsonResponse([]));
    });

    const client = new CellarrClient({ fetchImpl });
    render(<Notifications client={client} />);

    await waitFor(() => expect(screen.getByLabelText('Name')).toBeTruthy());
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Probe' } });
    fireEvent.click(screen.getByText('Test'));

    await waitFor(() => {
      const testCall = fetchImpl.mock.calls.find(([url]) =>
        String(url).endsWith('/api/v3/notification/test')
      );
      expect(testCall).toBeTruthy();
    });
    await waitFor(() => expect(screen.getByText(/test delivered/i)).toBeTruthy());
  });

  it('confirms before deleting then DELETEs the notification', async () => {
    let deleted = false;
    const fetchImpl = vi.fn().mockImplementation((url: string, opts?: RequestInit) => {
      const u = String(url);
      if (u.endsWith('/api/v3/notification/schema')) return Promise.resolve(jsonResponse(SCHEMA));
      if (u.includes('/api/v3/notification/') && opts?.method === 'DELETE') {
        deleted = true;
        return Promise.resolve(new Response(null, { status: 200 }));
      }
      if (u.endsWith('/api/v3/notification'))
        return Promise.resolve(jsonResponse(deleted ? [] : NOTIFICATIONS));
      return Promise.resolve(jsonResponse([]));
    });

    const client = new CellarrClient({ fetchImpl });
    render(<Notifications client={client} />);

    await waitFor(() => expect(screen.getByLabelText('Remove My Discord')).toBeTruthy());
    fireEvent.click(screen.getByLabelText('Remove My Discord'));
    // No DELETE before confirming.
    expect(fetchImpl.mock.calls.find(([, o]) => o?.method === 'DELETE')).toBeFalsy();
    await waitFor(() => expect(screen.getByRole('alertdialog')).toBeTruthy());

    fireEvent.click(screen.getByRole('button', { name: 'Remove notification' }));
    await waitFor(() => {
      const del = fetchImpl.mock.calls.find(
        ([url, o]) => String(url).endsWith('/api/v3/notification/3') && o?.method === 'DELETE'
      );
      expect(del).toBeTruthy();
    });
  });
});
