import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import { ThemeProvider } from '@lib/ThemeProvider';
import { ToastProvider } from '@app/_lib/ToastProvider';
import ImportLists from '@app/settings/_components/ImportLists';

// The component reads several surfaces in parallel (importlist, schema,
// qualityprofile, libraries, exclusions); route the fetch mock by URL+method so
// call ordering does not matter.
function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const SCHEMA = [
  {
    name: '',
    implementation: 'TMDbListImport',
    implementationName: 'TMDbListImport',
    configContract: 'TMDbListImportSettings',
    fields: [
      { order: 0, name: 'shouldMonitor', label: 'Monitor', type: 'checkbox', value: true },
      { order: 1, name: 'cleanLibraryLevel', label: 'Clean Library', type: 'select', value: 'disabled' },
      { order: 10, name: 'api_key', label: 'TMDb API Key', type: 'textbox', privacy: 'apiKey' },
      { order: 11, name: 'list_id', label: 'List ID', type: 'textbox' },
    ],
    presets: [],
    tags: [],
  },
  {
    name: '',
    implementation: 'TraktList',
    implementationName: 'TraktList',
    configContract: 'TraktListSettings',
    fields: [
      { order: 0, name: 'shouldMonitor', label: 'Monitor', type: 'checkbox', value: true },
      { order: 1, name: 'cleanLibraryLevel', label: 'Clean Library', type: 'select', value: 'disabled' },
      { order: 10, name: 'client_id', label: 'Client ID', type: 'textbox', privacy: 'apiKey' },
      { order: 11, name: 'list', label: 'List Slug', type: 'textbox' },
    ],
    presets: [],
    tags: [],
  },
];

function routedFetch(overrides: Record<string, () => Response> = {}) {
  return vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const method = (init?.method ?? 'GET').toUpperCase();
    const key = `${method} ${url.replace(/\?.*$/, '')}`;
    const handler = Object.entries(overrides).find(([k]) => key.endsWith(k))?.[1];
    if (handler) return Promise.resolve(handler());
    // Defaults for the parallel initial loads.
    if (url.endsWith('/importlist/schema')) return Promise.resolve(jsonResponse(SCHEMA));
    if (url.endsWith('/importlistexclusion') && method === 'GET') return Promise.resolve(jsonResponse([]));
    if (url.endsWith('/importlist') && method === 'GET') return Promise.resolve(jsonResponse([]));
    if (url.endsWith('/qualityprofile')) return Promise.resolve(jsonResponse([]));
    if (url.endsWith('/libraries')) return Promise.resolve(jsonResponse([]));
    return Promise.resolve(jsonResponse({}));
  });
}

const renderWith = (client: CellarrClient) =>
  render(
    <ThemeProvider>
      <ToastProvider>
        <ImportLists client={client} />
      </ToastProvider>
    </ThemeProvider>
  );

describe('ImportLists (settings)', () => {
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

  it('renders the new-import-list form with per-type schema fields', async () => {
    const fetchImpl = routedFetch();
    renderWith(new CellarrClient({ fetchImpl }));
    await waitFor(() => expect(screen.getByLabelText('Import list name')).toBeTruthy());
    // The default (first) schema type is TMDb, so its api_key + list_id show.
    await waitFor(() => expect(screen.getByLabelText('TMDb API Key')).toBeTruthy());
    expect(await screen.findByLabelText('List ID')).toBeTruthy();
  });

  it('POSTs a new import list with the chosen type + settings fields', async () => {
    const fetchImpl = routedFetch({
      'POST /api/v3/importlist': () => jsonResponse({ id: 7 }),
    });
    renderWith(new CellarrClient({ fetchImpl }));
    // Wait for the per-type schema fields to render (the schema loads via an async
    // effect) BEFORE interacting — firing input changes while the parallel data
    // loaders are still in flight can perturb the in-flight load under full-suite
    // CPU contention. Once a schema field is present the form has fully settled.
    await waitFor(() => expect(screen.getByLabelText('TMDb API Key')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Import list name'), {
      target: { value: 'Trending' },
    });
    fireEvent.change(screen.getByLabelText('TMDb API Key'), {
      target: { value: 'abc123' },
    });
    fireEvent.change(screen.getByLabelText('List ID'), {
      target: { value: '42' },
    });
    fireEvent.click(screen.getByText('Save'));

    await waitFor(() => {
      const post = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).endsWith('/api/v3/importlist') &&
          (opts as RequestInit)?.method === 'POST'
      );
      expect(post).toBeTruthy();
      const body = JSON.parse((post![1] as RequestInit).body as string);
      expect(body.name).toBe('Trending');
      expect(body.implementation).toBe('TMDbListImport');
      // The reserved flags ride at the top level; the source settings ride in fields[].
      expect(body.cleanLibraryLevel).toBe('disabled');
      const fieldNames = body.fields.map((f: { name: string }) => f.name);
      expect(fieldNames).toContain('api_key');
      expect(fieldNames).toContain('list_id');
      const apiKey = body.fields.find((f: { name: string }) => f.name === 'api_key');
      expect(apiKey.value).toBe('abc123');
    });
  });

  it('triggers a sync for an existing list', async () => {
    const LISTS = [
      {
        id: 5,
        name: 'My List',
        implementation: 'TMDbListImport',
        implementationName: 'TMDbListImport',
        configContract: 'TMDbListImportSettings',
        enabled: true,
        enableAuto: true,
        monitor: 'all',
        shouldMonitor: true,
        cleanLibraryLevel: 'disabled',
        lastSuccessfulSync: null,
        fields: [],
        tags: [],
      },
    ];
    const fetchImpl = routedFetch({
      'GET /api/v3/importlist': () => jsonResponse(LISTS),
      'POST /api/v3/importlist/5/sync': () =>
        jsonResponse({
          triggered: true,
          lists: [{ id: 5, name: 'My List', fetchSucceeded: true, added: 3, cleaned: 0 }],
        }),
    });
    renderWith(new CellarrClient({ fetchImpl }));
    await waitFor(() => expect(screen.getByLabelText('Sync My List now')).toBeTruthy());

    fireEvent.click(screen.getByLabelText('Sync My List now'));
    await waitFor(() => {
      const sync = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).endsWith('/api/v3/importlist/5/sync') &&
          (opts as RequestInit)?.method === 'POST'
      );
      expect(sync).toBeTruthy();
    });
  });

  it('POSTs a new exclusion with the chosen id type', async () => {
    const fetchImpl = routedFetch({
      'POST /api/v3/importlistexclusion': () => jsonResponse({ id: 1, tmdbId: 603 }),
    });
    renderWith(new CellarrClient({ fetchImpl }));
    await waitFor(() => expect(screen.getByLabelText('Exclusion external id')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Exclusion external id'), {
      target: { value: '603' },
    });
    fireEvent.click(screen.getByText('Add exclusion'));

    await waitFor(() => {
      const post = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).endsWith('/api/v3/importlistexclusion') &&
          (opts as RequestInit)?.method === 'POST'
      );
      expect(post).toBeTruthy();
      const body = JSON.parse((post![1] as RequestInit).body as string);
      expect(body.tmdbId).toBe('603');
    });
  });
});
