import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { ThemeProvider } from '@lib/ThemeProvider';
import { ModalProvider } from '@components/page/ModalContext';
import { HotkeysProvider } from '@modules/hotkeys';

// The Add screen talks to the shared client's generic `request`; mock it so the
// component test is hermetic.
const request = vi.fn();
vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: { request: (...args: unknown[]) => request(...args) },
  };
});

import AddPage from '@app/add/page';

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
      <HotkeysProvider>
        <ModalProvider>
          <AddPage />
        </ModalProvider>
      </HotkeysProvider>
    </ThemeProvider>
  );
}

describe('Add / search-new screen', () => {
  beforeEach(() => {
    request.mockReset();
    window.localStorage.clear();
    document.body.className = '';
    installMatchMedia();
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('shows the idle prompt before any search', () => {
    renderPage();
    expect(screen.getByText(/Start typing above/i)).toBeTruthy();
  });

  it('looks up titles on input and renders a results table', async () => {
    request.mockResolvedValue([
      { foreign_id: 'tt1', title: 'Blade Runner', year: 1982, media_type: 'movie', overview: 'A blade runner.' },
    ]);
    const { container } = renderPage();

    const input = container.querySelector('input[name="add-search"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'blade' } });

    // Wait out the real debounce + fetch.
    await waitFor(
      () =>
        expect(request).toHaveBeenCalledWith(
          '/lookup',
          expect.objectContaining({ query: { term: 'blade' } })
        ),
      { timeout: 2000 }
    );
    await waitFor(() => expect(screen.getByText('Blade Runner')).toBeTruthy());
    expect(screen.getByText('1982')).toBeTruthy();
  });

  it('opens a confirm dialog and posts the add on confirm', async () => {
    request
      .mockResolvedValueOnce([{ foreign_id: 'tt1', title: 'Blade Runner', year: 1982, media_type: 'movie' }])
      .mockResolvedValueOnce({ id: 'c1' });
    const { container } = renderPage();

    const input = container.querySelector('input[name="add-search"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'blade' } });
    await waitFor(() => expect(screen.getByText('Blade Runner')).toBeTruthy(), { timeout: 2000 });

    // Click the per-row Add ActionButton (avoid the "Search" button etc).
    const addButton = screen
      .getAllByRole('button')
      .find((el) => el.textContent?.includes('Add')) as HTMLElement;
    fireEvent.click(addButton);
    // Dialog appears with the title.
    await waitFor(() => expect(screen.getByText(/Add "Blade Runner"/)).toBeTruthy());

    fireEvent.click(screen.getByText('OK'));
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith('/content', expect.objectContaining({ method: 'POST' }))
    );
    await waitFor(() => expect(screen.getByText('added')).toBeTruthy());
  });

  it('renders an error banner when lookup fails', async () => {
    request.mockRejectedValue(new Error('boom'));
    const { container } = renderPage();
    const input = container.querySelector('input[name="add-search"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'x' } });
    await waitFor(() => expect(screen.getByText(/Search failed/i)).toBeTruthy(), { timeout: 2000 });
  });
});
