import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import CustomFormats from '@app/settings/_components/CustomFormats';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const FORMATS = [
  {
    id: 'x265',
    name: 'x265',
    score: 25,
    conditions: [{ type: 'Release Title', value: 'x265' }],
  },
  {
    id: 'remux',
    name: 'Remux',
    score: 100,
    conditions: [{ type: 'Source', value: 'remux' }],
  },
];

describe('CustomFormats (settings)', () => {
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

  it('renders the formats table', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse(FORMATS));
    const client = new CellarrClient({ fetchImpl });
    render(<CustomFormats client={client} />);
    await waitFor(() => expect(screen.getAllByText('x265').length).toBeGreaterThan(0));
    expect(screen.getByText('Remux')).toBeTruthy();
    expect(screen.getByText('+25')).toBeTruthy();
  });

  it('filters by name', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse(FORMATS));
    const client = new CellarrClient({ fetchImpl });
    render(<CustomFormats client={client} />);
    await waitFor(() => expect(screen.getAllByText('x265').length).toBeGreaterThan(0));

    fireEvent.change(screen.getByLabelText('Filter custom formats'), {
      target: { value: 'remux' },
    });
    expect(screen.queryByText('x265')).toBeNull();
    expect(screen.getByText('Remux')).toBeTruthy();
  });

  it('opens the add dialog and POSTs a new format', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(FORMATS))
      .mockResolvedValueOnce(jsonResponse({ id: 'new', name: 'HDR' }))
      .mockResolvedValueOnce(jsonResponse(FORMATS));
    const client = new CellarrClient({ fetchImpl });
    render(<CustomFormats client={client} />);
    await waitFor(() => expect(screen.getAllByText('x265').length).toBeGreaterThan(0));

    fireEvent.click(screen.getByText('Add custom format'));
    await waitFor(() => expect(screen.getByLabelText('Custom format name')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Custom format name'), { target: { value: 'HDR' } });
    fireEvent.change(screen.getByLabelText('Score'), { target: { value: '40' } });
    // Dialog's own confirm button (OK) drives save.
    fireEvent.click(screen.getByText('OK'));

    await waitFor(() => {
      const postCall = fetchImpl.mock.calls.find(
        ([url, opts]) => String(url).endsWith('/customformats') && opts?.method === 'POST'
      );
      expect(postCall).toBeTruthy();
      const body = JSON.parse((postCall![1] as RequestInit).body as string);
      expect(body.name).toBe('HDR');
      expect(body.score).toBe(40);
    });
  });

  it('shows an empty state with no formats', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse([]));
    const client = new CellarrClient({ fetchImpl });
    render(<CustomFormats client={client} />);
    await waitFor(() =>
      expect(screen.getByText(/no custom formats yet/i)).toBeTruthy()
    );
  });
});
