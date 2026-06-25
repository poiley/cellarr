import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import QualityDefinitions from '@app/settings/_components/QualityDefinitions';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const BYTES_PER_MB = 1024 * 1024;

// Two definitions: SDTV (min 2 MB/min, no max) and WEBDL-1080p (min 5 MB/min,
// max 100 MB/min). Sizes are bytes-per-minute on the wire.
const DEFINITIONS = [
  {
    id: 1,
    title: 'SDTV',
    weight: 1,
    minSize: 2 * BYTES_PER_MB,
    maxSize: null,
    preferredSize: null,
    quality: { id: 0, name: 'SDTV', source: 'tv', resolution: 480 },
  },
  {
    id: 2,
    title: 'WEBDL-1080p',
    weight: 2,
    minSize: 5 * BYTES_PER_MB,
    maxSize: 100 * BYTES_PER_MB,
    preferredSize: 50 * BYTES_PER_MB,
    quality: { id: 1, name: 'WEBDL-1080p', source: 'web', resolution: 1080 },
  },
];

function routedFetch(extra?: (url: string, opts?: RequestInit) => Response | undefined) {
  return vi.fn((url: string, opts?: RequestInit) => {
    const u = String(url);
    const method = opts?.method ?? 'GET';
    const fromExtra = extra?.(u, opts);
    if (fromExtra) return Promise.resolve(fromExtra);
    if (u.endsWith('/api/v3/qualitydefinition') && method === 'GET')
      return Promise.resolve(jsonResponse(DEFINITIONS));
    return Promise.resolve(jsonResponse({}));
  });
}

describe('QualityDefinitions (settings)', () => {
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

  it('loads definitions and seeds the size fields in MB/min', async () => {
    const client = new CellarrClient({ fetchImpl: routedFetch() as never });
    render(<QualityDefinitions client={client} />);

    await waitFor(() =>
      expect(screen.getByLabelText('Title for SDTV')).toBeTruthy()
    );

    // bytes-per-minute -> MB/min at the edge.
    expect(
      (screen.getByLabelText(/Minimum size for SDTV/) as HTMLInputElement).value
    ).toBe('2');
    // No max on SDTV -> empty field.
    expect(
      (screen.getByLabelText(/Maximum size for SDTV/) as HTMLInputElement).value
    ).toBe('');
    expect(
      (screen.getByLabelText(/Minimum size for WEBDL-1080p/) as HTMLInputElement).value
    ).toBe('5');
    expect(
      (screen.getByLabelText(/Maximum size for WEBDL-1080p/) as HTMLInputElement).value
    ).toBe('100');
  });

  it('Save is disabled until a bound is edited, then bulk-PUTs the changed rows in bytes-per-minute', async () => {
    const fetchImpl = routedFetch((u, opts) => {
      if (u.endsWith('/api/v3/qualitydefinition/update') && opts?.method === 'PUT') {
        return jsonResponse(DEFINITIONS);
      }
      return undefined;
    });
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<QualityDefinitions client={client} />);

    await waitFor(() =>
      expect(screen.getByLabelText('Title for WEBDL-1080p')).toBeTruthy()
    );

    // Nothing edited yet -> the SRCL Button renders its disabled (non-button)
    // variant as a <div>, so clicking it is inert.
    expect(screen.getByText('Save definitions').tagName).toBe('DIV');

    // Edit WEBDL-1080p: min 5 -> 8 MB/min, max 100 -> 120 MB/min.
    fireEvent.change(screen.getByLabelText(/Minimum size for WEBDL-1080p/), {
      target: { value: '8' },
    });
    fireEvent.change(screen.getByLabelText(/Maximum size for WEBDL-1080p/), {
      target: { value: '120' },
    });

    fireEvent.click(screen.getByText('Save definitions'));

    await waitFor(() => {
      const put = fetchImpl.mock.calls.find(
        ([url, o]: [string, RequestInit | undefined]) =>
          String(url).endsWith('/api/v3/qualitydefinition/update') && o?.method === 'PUT'
      );
      expect(put).toBeTruthy();
      const body = JSON.parse((put![1] as RequestInit).body as string);
      // Only the one dirty row is sent, addressed by its id.
      expect(Array.isArray(body)).toBe(true);
      expect(body).toHaveLength(1);
      expect(body[0].id).toBe(2);
      // MB/min -> bytes-per-minute on the wire.
      expect(body[0].minSize).toBe(8 * BYTES_PER_MB);
      expect(body[0].maxSize).toBe(120 * BYTES_PER_MB);
    });
  });

  it('clearing the Max field persists a null (no upper bound)', async () => {
    const fetchImpl = routedFetch((u, opts) => {
      if (u.endsWith('/api/v3/qualitydefinition/update') && opts?.method === 'PUT') {
        return jsonResponse(DEFINITIONS);
      }
      return undefined;
    });
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<QualityDefinitions client={client} />);

    await waitFor(() =>
      expect(screen.getByLabelText('Title for WEBDL-1080p')).toBeTruthy()
    );

    fireEvent.change(screen.getByLabelText(/Maximum size for WEBDL-1080p/), {
      target: { value: '' },
    });
    fireEvent.click(screen.getByText('Save definitions'));

    await waitFor(() => {
      const put = fetchImpl.mock.calls.find(
        ([url, o]: [string, RequestInit | undefined]) =>
          String(url).endsWith('/api/v3/qualitydefinition/update') && o?.method === 'PUT'
      );
      expect(put).toBeTruthy();
      const body = JSON.parse((put![1] as RequestInit).body as string);
      expect(body[0].id).toBe(2);
      expect(body[0].maxSize).toBeNull();
    });
  });

  it('shows an empty state when there are no definitions', async () => {
    const fetchImpl = vi.fn(() => Promise.resolve(jsonResponse([])));
    const client = new CellarrClient({ fetchImpl: fetchImpl as never });
    render(<QualityDefinitions client={client} />);
    await waitFor(() => expect(screen.getByText(/no quality definitions yet/i)).toBeTruthy());
  });
});
