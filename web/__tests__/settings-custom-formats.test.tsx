import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import CustomFormats from '@app/settings/_components/CustomFormats';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

// The v3 customformat list shape: numeric id + specifications[].
const FORMATS = [
  {
    id: 1,
    name: 'x265',
    includeCustomFormatWhenRenaming: false,
    specifications: [
      {
        name: 'x265-1',
        implementation: 'ReleaseTitleSpecification',
        negate: false,
        required: false,
        fields: [{ name: 'value', value: 'x265' }],
      },
    ],
  },
  {
    id: 2,
    name: 'Remux',
    includeCustomFormatWhenRenaming: false,
    specifications: [
      {
        name: 'Remux-1',
        implementation: 'SourceSpecification',
        negate: false,
        required: false,
        fields: [{ name: 'value', value: 'remux' }],
      },
    ],
  },
];

// The schema the spec-row builder is driven by.
const SCHEMA = [
  {
    implementation: 'ReleaseTitleSpecification',
    implementationName: 'Release Title',
    negate: false,
    required: false,
    fields: [{ order: 0, name: 'value', label: 'Release Title', type: 'textbox' }],
    presets: [],
  },
  {
    implementation: 'SourceSpecification',
    implementationName: 'Source',
    negate: false,
    required: false,
    fields: [
      {
        order: 0,
        name: 'value',
        label: 'Source',
        type: 'select',
        selectOptions: [
          { value: 'web-dl', name: 'web-dl', order: 0 },
          { value: 'remux', name: 'remux', order: 1 },
        ],
      },
    ],
    presets: [],
  },
];

// Route a request by url + method to the right canned response.
function routedFetch(extra?: (url: string, opts?: RequestInit) => Response | undefined) {
  return vi.fn((url: string, opts?: RequestInit) => {
    const u = String(url);
    const method = opts?.method ?? 'GET';
    const fromExtra = extra?.(u, opts);
    if (fromExtra) return Promise.resolve(fromExtra);
    if (u.endsWith('/api/v3/customformat/schema')) return Promise.resolve(jsonResponse(SCHEMA));
    if (u.endsWith('/api/v3/customformat') && method === 'GET')
      return Promise.resolve(jsonResponse(FORMATS));
    return Promise.resolve(jsonResponse({}));
  });
}

describe('CustomFormats (settings editor)', () => {
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

  it('lists formats with Edit / Delete and filters by name', async () => {
    const client = new CellarrClient({ fetchImpl: routedFetch() as never });
    render(<CustomFormats client={client} />);
    await waitFor(() => expect(screen.getByText('x265')).toBeTruthy());
    expect(screen.getByText('Remux')).toBeTruthy();
    expect(screen.getByLabelText('Edit x265')).toBeTruthy();
    expect(screen.getByLabelText('Delete x265')).toBeTruthy();

    fireEvent.change(screen.getByLabelText('Filter custom formats'), {
      target: { value: 'remux' },
    });
    expect(screen.queryByText('x265')).toBeNull();
    expect(screen.getByText('Remux')).toBeTruthy();
  });

  it('builds and POSTs a multi-spec custom format', async () => {
    const fetchImpl = routedFetch((u, opts) => {
      if (u.endsWith('/api/v3/customformat') && (opts?.method ?? 'GET') === 'POST') {
        return jsonResponse({ id: 9, name: 'HDR Web', specifications: [] });
      }
      return undefined;
    });
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<CustomFormats client={client} />);
    await waitFor(() => expect(screen.getByText('x265')).toBeTruthy());

    fireEvent.click(screen.getByText('Add custom format'));
    await waitFor(() => expect(screen.getByLabelText('Custom format name')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Custom format name'), { target: { value: 'HDR Web' } });

    // First spec: a Release Title regex (the default implementation).
    fireEvent.change(screen.getByLabelText('Specification 1 value'), { target: { value: 'HDR' } });

    // Add a second spec.
    fireEvent.click(screen.getByLabelText('Add specification'));
    await waitFor(() => expect(screen.getByLabelText('Specification 2 value')).toBeTruthy());
    fireEvent.change(screen.getByLabelText('Specification 2 value'), { target: { value: 'WEB' } });

    fireEvent.click(screen.getByText('Create format'));

    await waitFor(() => {
      const postCall = fetchImpl.mock.calls.find(
        ([url, o]: [string, RequestInit | undefined]) =>
          String(url).endsWith('/api/v3/customformat') && o?.method === 'POST'
      );
      expect(postCall).toBeTruthy();
      const body = JSON.parse((postCall![1] as RequestInit).body as string);
      expect(body.name).toBe('HDR Web');
      expect(body.specifications).toHaveLength(2);
      expect(body.specifications[0].implementation).toBe('ReleaseTitleSpecification');
      expect(body.specifications[0].fields[0].value).toBe('HDR');
      expect(body.specifications[1].fields[0].value).toBe('WEB');
    });
  });

  it('runs a live test preview against POST /customformat/test', async () => {
    const fetchImpl = routedFetch((u, opts) => {
      if (u.endsWith('/api/v3/customformat/test') && (opts?.method ?? 'GET') === 'POST') {
        return jsonResponse([
          { id: 1, name: 'x265', matched: true, score: 25 },
          { id: 2, name: 'Remux', matched: false, score: 100 },
        ]);
      }
      return undefined;
    });
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<CustomFormats client={client} />);
    await waitFor(() => expect(screen.getByText('x265')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Test release title'), {
      target: { value: 'The.Movie.2024.1080p.x265-GRP' },
    });
    fireEvent.click(screen.getByLabelText('Run test'));

    await waitFor(() => {
      const testCall = fetchImpl.mock.calls.find(
        ([url, o]: [string, RequestInit | undefined]) =>
          String(url).endsWith('/api/v3/customformat/test') && o?.method === 'POST'
      );
      expect(testCall).toBeTruthy();
      const body = JSON.parse((testCall![1] as RequestInit).body as string);
      expect(body.title).toBe('The.Movie.2024.1080p.x265-GRP');
    });

    await waitFor(() => expect(screen.getByText('✓ match')).toBeTruthy());
    expect(screen.getByText('✗ no')).toBeTruthy();
  });

  it('deletes a format through the confirm dialog', async () => {
    const fetchImpl = routedFetch((u, opts) => {
      if (/\/api\/v3\/customformat\/1$/.test(u) && opts?.method === 'DELETE') {
        return jsonResponse({});
      }
      return undefined;
    });
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<CustomFormats client={client} />);
    await waitFor(() => expect(screen.getByText('x265')).toBeTruthy());

    fireEvent.click(screen.getByLabelText('Delete x265'));
    // The ConfirmDialog danger button shares the aria-label with the trigger; the
    // dialog's button is the one inside role="alertdialog".
    await waitFor(() => expect(screen.getByRole('alertdialog')).toBeTruthy());
    const dialog = screen.getByRole('alertdialog');
    const confirmBtn = within(dialog).getByLabelText('Delete custom format');
    fireEvent.click(confirmBtn);

    await waitFor(() => {
      const del = fetchImpl.mock.calls.find(
        ([url, o]: [string, RequestInit | undefined]) =>
          /\/api\/v3\/customformat\/1$/.test(String(url)) && o?.method === 'DELETE'
      );
      expect(del).toBeTruthy();
    });
  });

  it('shows an empty state with no formats', async () => {
    const fetchImpl = vi.fn((url: string) => {
      const u = String(url);
      if (u.endsWith('/api/v3/customformat/schema')) return Promise.resolve(jsonResponse(SCHEMA));
      return Promise.resolve(jsonResponse([]));
    });
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<CustomFormats client={client} />);
    await waitFor(() => expect(screen.getByText(/no custom formats yet/i)).toBeTruthy());
  });
});
