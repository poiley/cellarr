import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import Naming from '@app/settings/_components/Naming';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const NAMING = {
  movieFileFormat: '{Movie Title} ({Release Year})/{Movie Title}.{Extension}',
  seriesFolderFormat: '{Series Title}',
  seasonFolderFormat: 'Season {Season}',
  episodeFileFormat: '{Series Title} - S{Season}E{Episode}.{Extension}',
  renameEpisodes: true,
  renameMovies: true,
  seasonFolders: true,
};

const TOKENS = {
  targets: [
    {
      target: 'movieFile',
      tokens: [
        { token: '{Movie Title}', name: 'Movie Title', label: 'Movie Title', required: true, example: 'Blade Runner' },
        { token: '{Extension}', name: 'Extension', label: 'Extension', required: false, example: 'mkv' },
      ],
    },
    { target: 'seriesFolder', tokens: [] },
    { target: 'seasonFolder', tokens: [] },
    { target: 'episodeFile', tokens: [] },
  ],
};

const MEDIA_MGMT = {
  permissions: { chmodFolder: '755', chmodFile: '644', chown: 'media:media' },
  extraFiles: { enabled: false, extensions: ['srt', 'nfo'] },
};

// The component fires three loads on mount (naming / tokens / mediamanagement)
// and POSTs naming previews as formats change. Route every fetch by URL+method
// so the test is order-independent; `rendered` echoes the requested format so
// each preview line is identifiable.
function routedFetch(extra?: (url: string, opts?: RequestInit) => Response | undefined) {
  return vi.fn().mockImplementation((url: string, opts?: RequestInit) => {
    const u = String(url);
    const override = extra?.(u, opts);
    if (override) return Promise.resolve(override);
    if (u.endsWith('/api/v3/config/naming/tokens')) return Promise.resolve(jsonResponse(TOKENS));
    if (u.endsWith('/api/v3/config/naming/preview')) {
      const body = JSON.parse((opts?.body as string) ?? '{}');
      return Promise.resolve(jsonResponse({ format: body.format, target: body.target, rendered: `RENDERED:${body.format}` }));
    }
    if (u.endsWith('/api/v3/config/naming')) return Promise.resolve(jsonResponse(NAMING));
    if (u.endsWith('/api/v3/config/mediamanagement')) return Promise.resolve(jsonResponse(MEDIA_MGMT));
    return Promise.resolve(jsonResponse({}));
  });
}

describe('Naming (settings)', () => {
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

  it('loads the config and renders a live preview that updates as the format changes', async () => {
    const fetchImpl = routedFetch();
    const client = new CellarrClient({ fetchImpl });
    render(<Naming client={client} />);

    // Initial preview renders the loaded movie-file format.
    await waitFor(() =>
      expect(
        screen.getByText(`RENDERED:${NAMING.movieFileFormat}`)
      ).toBeTruthy()
    );

    // Editing the format re-POSTs the preview (debounced) and re-renders.
    fireEvent.change(screen.getByLabelText('Movie file format'), {
      target: { value: '{Movie Title}.{Extension}' },
    });

    await waitFor(() => {
      expect(screen.getByText('RENDERED:{Movie Title}.{Extension}')).toBeTruthy();
      const previews = fetchImpl.mock.calls.filter(
        ([u, o]) => String(u).endsWith('/api/v3/config/naming/preview') && (o as RequestInit)?.method === 'POST'
      );
      expect(previews.length).toBeGreaterThan(0);
    });
  });

  it('inserts a token on click then PUTs the naming config on save', async () => {
    const fetchImpl = routedFetch();
    const client = new CellarrClient({ fetchImpl });
    render(<Naming client={client} />);

    await waitFor(() => expect(screen.getByLabelText('Movie file format')).toBeTruthy());

    // Click-to-insert appends the token to the field.
    fireEvent.change(screen.getByLabelText('Movie file format'), { target: { value: 'X' } });
    fireEvent.click(
      screen.getByLabelText('Insert Extension token into Movie file format')
    );
    await waitFor(() =>
      expect((screen.getByLabelText('Movie file format') as HTMLInputElement).value).toBe('X{Extension}')
    );

    fireEvent.click(screen.getByText('Save naming'));

    await waitFor(() => {
      const put = fetchImpl.mock.calls.find(
        ([u, o]) =>
          String(u).endsWith('/api/v3/config/naming') && (o as RequestInit)?.method === 'PUT'
      );
      expect(put).toBeTruthy();
      const body = JSON.parse((put![1] as RequestInit).body as string);
      expect(body.movieFileFormat).toBe('X{Extension}');
      expect(body.seriesFolderFormat).toBe(NAMING.seriesFolderFormat);
    });
  });

  it('saves permissions to the media-management blob', async () => {
    const fetchImpl = routedFetch();
    const client = new CellarrClient({ fetchImpl });
    render(<Naming client={client} />);

    await waitFor(() => expect(screen.getByLabelText('chmod folder')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('chmod folder'), { target: { value: '770' } });
    fireEvent.click(screen.getByText('Save permissions'));

    await waitFor(() => {
      const put = fetchImpl.mock.calls.find(
        ([u, o]) =>
          String(u).endsWith('/api/v3/config/mediamanagement') &&
          (o as RequestInit)?.method === 'PUT' &&
          JSON.parse(((o as RequestInit)?.body as string) ?? '{}').permissions
      );
      expect(put).toBeTruthy();
      const body = JSON.parse((put![1] as RequestInit).body as string);
      expect(body.permissions.chmodFolder).toBe('770');
      expect(body.permissions.chmodFile).toBe('644');
    });
  });

  it('toggles "import extra files" and saves the extra-files config', async () => {
    const fetchImpl = routedFetch();
    const client = new CellarrClient({ fetchImpl });
    render(<Naming client={client} />);

    const toggle = await waitFor(() => screen.getByLabelText('Import extra files'));
    expect(toggle.getAttribute('aria-checked')).toBe('false');

    fireEvent.click(toggle);
    await waitFor(() =>
      expect(screen.getByLabelText('Import extra files').getAttribute('aria-checked')).toBe('true')
    );

    // Add a new extension, remove a pre-loaded one.
    fireEvent.change(screen.getByLabelText('New extension'), { target: { value: '.ass' } });
    fireEvent.click(screen.getByText('Add'));
    fireEvent.click(screen.getByLabelText('Remove extension nfo'));

    fireEvent.click(screen.getByText('Save extra files'));

    await waitFor(() => {
      const put = fetchImpl.mock.calls.find(
        ([u, o]) =>
          String(u).endsWith('/api/v3/config/mediamanagement') &&
          (o as RequestInit)?.method === 'PUT' &&
          JSON.parse(((o as RequestInit)?.body as string) ?? '{}').extraFiles
      );
      expect(put).toBeTruthy();
      const body = JSON.parse((put![1] as RequestInit).body as string);
      expect(body.extraFiles.enabled).toBe(true);
      expect(body.extraFiles.extensions).toContain('srt');
      expect(body.extraFiles.extensions).toContain('ass');
      expect(body.extraFiles.extensions).not.toContain('nfo');
    });
  });
});
