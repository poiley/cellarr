import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor, fireEvent } from '@testing-library/react';

// --- mocks ------------------------------------------------------------------
// The Collections screen reads the singleton API client (listCollections +
// updateCollection) and the toast API. We mock both so the component can be
// exercised in jsdom and we can assert the monitor toggle PUTs.

const listCollections = vi.fn();
const updateCollection = vi.fn();

vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      listCollections: (...a: unknown[]) => listCollections(...a),
      updateCollection: (...a: unknown[]) => updateCollection(...a),
    },
  };
});

const toastSuccess = vi.fn();
const toastError = vi.fn();
const toastInfo = vi.fn();

vi.mock('@app/_lib/ToastProvider', async () => {
  const actual = await vi.importActual<typeof import('@app/_lib/ToastProvider')>(
    '@app/_lib/ToastProvider'
  );
  return {
    ...actual,
    useToast: () => ({
      success: toastSuccess,
      error: toastError,
      info: toastInfo,
      push: vi.fn(),
      dismiss: vi.fn(),
    }),
  };
});

vi.mock('next/navigation', () => ({
  usePathname: () => '/collections',
  useRouter: () => ({ push: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
}));

import CollectionsPage from '@app/collections/page';
import { ThemeProvider } from '@lib/ThemeProvider';

function installMatchMedia() {
  window.matchMedia = vi.fn().mockReturnValue({
    matches: false,
    media: '(prefers-color-scheme: dark)',
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    onchange: null,
    dispatchEvent: () => true,
  }) as unknown as typeof window.matchMedia;
}

function renderPage() {
  return render(
    <ThemeProvider>
      <CollectionsPage />
    </ThemeProvider>
  );
}

const COLLECTIONS = [
  {
    id: 1,
    title: 'The Matrix Collection',
    tmdbId: 2344,
    monitored: true,
    qualityProfileId: 1,
    searchOnAdd: true,
    minimumAvailability: 'released',
    movies: [{ tmdbId: 603 }, { tmdbId: 604 }, { tmdbId: 605 }],
  },
  {
    id: 2,
    title: 'Blade Runner Collection',
    tmdbId: 422837,
    monitored: false,
    qualityProfileId: 1,
    searchOnAdd: false,
    minimumAvailability: 'released',
    movies: [{ tmdbId: 78 }],
  },
];

describe('Collections screen', () => {
  beforeEach(() => {
    installMatchMedia();
    window.localStorage.clear();
    listCollections.mockReset();
    updateCollection.mockReset();
    updateCollection.mockResolvedValue(COLLECTIONS[0]);
    toastSuccess.mockReset();
    toastError.mockReset();
    toastInfo.mockReset();
  });
  afterEach(() => cleanup());

  it('shows an empty state for a face with no collections', async () => {
    listCollections.mockResolvedValue([]);
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/No collections\./i)).toBeTruthy();
    });
  });

  it('lists collections with their title, movie count, and monitored state', async () => {
    listCollections.mockResolvedValue(COLLECTIONS);
    renderPage();

    await waitFor(() => expect(listCollections).toHaveBeenCalled());
    await waitFor(() => {
      expect(screen.getByText('The Matrix Collection')).toBeTruthy();
      expect(screen.getByText('Blade Runner Collection')).toBeTruthy();
      // Member counts surfaced as badges.
      expect(screen.getByText('3 movies')).toBeTruthy();
      expect(screen.getByText('1 movie')).toBeTruthy();
      // Per-row monitored state.
      expect(screen.getByText('MONITORED')).toBeTruthy();
      expect(screen.getByText('UNMONITORED')).toBeTruthy();
    });
    // Header summary badge.
    expect(screen.getByText('1/2 monitored')).toBeTruthy();
  });

  it('PUTs monitored=true when an unmonitored collection is toggled on', async () => {
    listCollections.mockResolvedValue(COLLECTIONS);
    renderPage();

    await waitFor(() => expect(screen.getByText('Blade Runner Collection')).toBeTruthy());

    const toggle = screen.getByLabelText('Monitor Blade Runner Collection');
    fireEvent.click(toggle);

    await waitFor(() => {
      expect(updateCollection).toHaveBeenCalledWith(2, { monitored: true });
    });
    await waitFor(() => expect(toastSuccess).toHaveBeenCalled());
  });

  it('PUTs monitored=false when a monitored collection is toggled off', async () => {
    listCollections.mockResolvedValue(COLLECTIONS);
    renderPage();

    await waitFor(() => expect(screen.getByText('The Matrix Collection')).toBeTruthy());

    const toggle = screen.getByLabelText('Monitor The Matrix Collection');
    fireEvent.click(toggle);

    await waitFor(() => {
      expect(updateCollection).toHaveBeenCalledWith(1, { monitored: false });
    });
  });

  it('rolls the row back and surfaces an error toast when the PUT fails', async () => {
    listCollections.mockResolvedValue(COLLECTIONS);
    updateCollection.mockRejectedValue(new Error('boom'));
    renderPage();

    await waitFor(() => expect(screen.getByText('Blade Runner Collection')).toBeTruthy());

    const toggle = screen.getByLabelText('Monitor Blade Runner Collection');
    fireEvent.click(toggle);

    await waitFor(() => expect(updateCollection).toHaveBeenCalled());
    await waitFor(() => expect(toastError).toHaveBeenCalled());
    // The row reverts to its prior (unmonitored) state — still two distinct
    // status badges, one MONITORED (Matrix) and one UNMONITORED (Blade Runner).
    await waitFor(() => {
      expect(screen.getByText('MONITORED')).toBeTruthy();
      expect(screen.getByText('UNMONITORED')).toBeTruthy();
    });
  });

  it('filters collections by title', async () => {
    listCollections.mockResolvedValue(COLLECTIONS);
    renderPage();

    await waitFor(() => expect(screen.getByText('The Matrix Collection')).toBeTruthy());

    const filter = screen.getByLabelText('Filter');
    fireEvent.change(filter, { target: { value: 'matrix' } });

    await waitFor(() => {
      expect(screen.getByText('The Matrix Collection')).toBeTruthy();
      expect(screen.queryByText('Blade Runner Collection')).toBeNull();
    });
  });

  it('degrades to an empty list on a network error', async () => {
    const { ApiError } = await vi.importActual<typeof import('@lib/api/client')>(
      '@lib/api/client'
    );
    listCollections.mockRejectedValue(new ApiError('network_error', 'offline', 0));
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/No collections\./i)).toBeTruthy();
    });
  });
});
