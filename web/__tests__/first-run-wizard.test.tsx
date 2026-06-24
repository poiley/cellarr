import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { ModalProvider } from '@components/page/ModalContext';
import { ThemeProvider } from '@lib/ThemeProvider';
import { CellarrClient } from '@lib/api/client';

import WizardModal from '@app/first-run/_components/WizardModal';
import FirstRunPage from '@app/first-run/page';

const pushMock = vi.fn();
vi.mock('next/navigation', () => ({
  usePathname: () => '/',
  useRouter: () => ({ push: pushMock }),
}));

// The first-run page calls api.listLibraries() on mount to decide whether to
// show the wizard. Keep CellarrClient real (the wizard tests build their own),
// but stub the default `api.listLibraries`.
const listLibraries = vi.fn();
vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    // When the page launches the wizard via ModalTrigger, the wizard falls back
    // to this default `api`. Stub the methods it touches so that path works too
    // (the dedicated wizard tests inject their own CellarrClient instead).
    api: {
      listLibraries: (...args: unknown[]) => listLibraries(...args),
      getQualityProfiles: () => Promise.resolve([]),
      request: () => Promise.resolve({ id: 'x' }),
      createIndexer: () => Promise.resolve({ id: 1 }),
      createDownloadClient: () => Promise.resolve({ id: 1 }),
    },
  };
});

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

// A fetch stub that returns the seeded quality profiles for the profile GET and
// an echo body for everything else (the writes the wizard makes on finish).
function makeFetch() {
  return vi.fn((url: RequestInfo | URL, _init?: RequestInit) => {
    const u = String(url);
    if (u.endsWith('/qualityprofile')) {
      return Promise.resolve(
        jsonResponse([
          { id: 'qp-hd', name: 'HD-1080p' },
          { id: 'qp-web', name: 'WEB-1080p' },
        ])
      );
    }
    return Promise.resolve(jsonResponse({ id: 'x' }));
  });
}

describe('First-run wizard', () => {
  beforeEach(() => {
    window.localStorage.clear();
    document.body.className = '';
    pushMock.mockClear();
    listLibraries.mockReset();
    listLibraries.mockResolvedValue([]);
    // The wizard's step indicator uses BarProgress, which observes its
    // container width; jsdom has no ResizeObserver.
    (globalThis as Record<string, unknown>).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
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

  it('launches the wizard from the first-run page when no libraries exist', async () => {
    // The page checks for existing libraries on mount; return an empty list so
    // it shows the first-run prompt (rather than the "already set up" shortcut).
    listLibraries.mockResolvedValue([]);
    render(
      <ThemeProvider>
        <ModalProvider>
          <FirstRunPage />
        </ModalProvider>
      </ThemeProvider>
    );
    // The library check resolves async, then the prompt appears.
    await waitFor(() => expect(screen.getByText('Start setup')).toBeTruthy());
    // Before launch there is no modal dialog.
    expect(screen.queryByRole('dialog')).toBeNull();
    fireEvent.click(screen.getByText('Start setup'));
    // After launch the SRCL Dialog (the wizard) is on screen.
    await waitFor(() => expect(screen.getByRole('dialog')).toBeTruthy());
    expect(screen.getByText('Welcome')).toBeTruthy();
  });

  it('shows the already-set-up shortcut (no wizard prompt) when libraries exist', async () => {
    listLibraries.mockResolvedValue([{ id: 'lib-1', name: 'Movies', media_type: 'movie' }]);
    render(
      <ThemeProvider>
        <ModalProvider>
          <FirstRunPage />
        </ModalProvider>
      </ThemeProvider>
    );
    await waitFor(() => expect(screen.getByText('Go to Library')).toBeTruthy());
    // The first-run prompt is suppressed once a library exists.
    expect(screen.queryByText('Start setup')).toBeNull();
  });

  it('renders a step indicator inside the wizard', async () => {
    const fetchImpl = makeFetch();
    const client = new CellarrClient({ fetchImpl });
    render(
      <ModalProvider>
        <WizardModal client={client} />
      </ModalProvider>
    );
    // The step indicator is decorative (aria-hidden) but visible; it labels
    // every step and carries a BarProgress (progressbar role, hidden subtree).
    await waitFor(() =>
      expect(screen.getAllByRole('progressbar', { hidden: true }).length).toBeGreaterThan(0)
    );
    // All step labels appear in the indicator (Welcome..Finish).
    expect(screen.getAllByText(/Welcome/).length).toBeGreaterThan(0);
    expect(screen.getByText(/Finish/)).toBeTruthy();
  });

  it('is fully skippable from inside the wizard', async () => {
    const fetchImpl = makeFetch();
    const client = new CellarrClient({ fetchImpl });
    render(
      <ModalProvider>
        <WizardModal client={client} />
      </ModalProvider>
    );
    await waitFor(() =>
      expect(fetchImpl.mock.calls.some(([u]) => String(u).endsWith('/qualityprofile'))).toBe(true)
    );
    fireEvent.click(screen.getByText('Skip setup'));
    // Skipping creates nothing and routes to the Library screen.
    const wrote = fetchImpl.mock.calls.some(
      ([url, opts]) =>
        String(url).endsWith('/libraries') && (opts as RequestInit)?.method === 'POST'
    );
    expect(wrote).toBe(false);
    expect(pushMock).toHaveBeenCalledWith('/library/');
  });

  it('walks the steps and POSTs the library with a default quality profile on finish', async () => {
    const fetchImpl = makeFetch();
    const client = new CellarrClient({ fetchImpl });
    render(
      <ModalProvider>
        <WizardModal client={client} />
      </ModalProvider>
    );

    // Wait until the profile GET has resolved (default profile loaded).
    await waitFor(() =>
      expect(fetchImpl.mock.calls.some(([u]) => String(u).endsWith('/qualityprofile'))).toBe(true)
    );

    // Step 0 -> 1
    fireEvent.click(screen.getByText('Next'));
    expect(screen.getByLabelText('Library name')).toBeTruthy();

    fireEvent.change(screen.getByLabelText('Library name'), { target: { value: 'Films' } });
    fireEvent.change(screen.getByLabelText('Root folder'), { target: { value: '/m/films' } });

    // 1 -> 2 (indexer), 2 -> 3 (client), 3 -> 4 (finish)
    fireEvent.click(screen.getByText('Next'));
    fireEvent.click(screen.getByText('Next'));
    fireEvent.click(screen.getByText('Next'));

    expect(screen.getByText('Create library')).toBeTruthy();
    fireEvent.click(screen.getByText('Create library'));

    await waitFor(() => expect(screen.getByText(/setup complete/i)).toBeTruthy());
    const libCall = fetchImpl.mock.calls.find(
      ([url, opts]) => String(url).endsWith('/libraries') && (opts as RequestInit)?.method === 'POST'
    );
    expect(libCall).toBeTruthy();
    const body = JSON.parse((libCall![1] as RequestInit).body as string);
    expect(body.name).toBe('Films');
    expect(body.root_folders).toEqual(['/m/films']);
    // The library create must carry a default quality profile (daemon-required).
    expect(body.default_quality_profile).toBe('qp-hd');
  });

  it('also creates an indexer via the v3 endpoint when a host is provided', async () => {
    const fetchImpl = makeFetch();
    const client = new CellarrClient({ fetchImpl });
    render(
      <ModalProvider>
        <WizardModal client={client} />
      </ModalProvider>
    );

    fireEvent.click(screen.getByText('Next')); // -> Library
    fireEvent.change(screen.getByLabelText('Library name'), { target: { value: 'TV' } });
    fireEvent.change(screen.getByLabelText('Root folder'), { target: { value: '/m/tv' } });
    fireEvent.click(screen.getByText('Next')); // -> Indexer
    fireEvent.change(screen.getByLabelText('Indexer host'), {
      target: { value: 'http://idx:9117' },
    });
    fireEvent.click(screen.getByText('Next')); // -> Client
    fireEvent.click(screen.getByText('Next')); // -> Finish
    fireEvent.click(screen.getByText('Create library'));

    await waitFor(() => {
      const idxCall = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).endsWith('/api/v3/indexer') && (opts as RequestInit)?.method === 'POST'
      );
      expect(idxCall).toBeTruthy();
      const body = JSON.parse((idxCall![1] as RequestInit).body as string);
      // The v3 indexer carries its host inside the fields[] array (Radarr shape).
      const baseUrl = body.fields.find((f: { name: string }) => f.name === 'baseUrl');
      expect(baseUrl.value).toBe('http://idx:9117');
    });
  });

  it('skips optional integrations and routes to the Library on completion', async () => {
    const fetchImpl = makeFetch();
    const client = new CellarrClient({ fetchImpl });
    render(
      <ModalProvider>
        <WizardModal client={client} />
      </ModalProvider>
    );

    fireEvent.click(screen.getByText('Next')); // -> Library
    fireEvent.change(screen.getByLabelText('Library name'), { target: { value: 'Music' } });
    fireEvent.change(screen.getByLabelText('Root folder'), { target: { value: '/m/music' } });
    fireEvent.click(screen.getByText('Next')); // -> Indexer (left blank)
    fireEvent.click(screen.getByText('Next')); // -> Client (left blank)
    fireEvent.click(screen.getByText('Next')); // -> Finish
    fireEvent.click(screen.getByText('Create library'));

    await waitFor(() => expect(screen.getByText(/setup complete/i)).toBeTruthy());

    // No indexer / download-client POSTs should have been made.
    const wrote = (suffix: string) =>
      fetchImpl.mock.calls.some(
        ([url, opts]) =>
          String(url).endsWith(suffix) && (opts as RequestInit)?.method === 'POST'
      );
    expect(wrote('/api/v3/indexer')).toBe(false);
    expect(wrote('/api/v3/downloadclient')).toBe(false);

    fireEvent.click(screen.getByText('Go to Library'));
    expect(pushMock).toHaveBeenCalledWith('/library/');
  });
});
