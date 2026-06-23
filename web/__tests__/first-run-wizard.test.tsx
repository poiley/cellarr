import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { ModalProvider } from '@components/page/ModalContext';
import { ThemeProvider } from '@lib/ThemeProvider';
import { CellarrClient } from '@lib/api/client';

import WizardModal from '@app/first-run/_components/WizardModal';
import FirstRunPage from '@app/first-run/page';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

describe('First-run wizard', () => {
  beforeEach(() => {
    window.localStorage.clear();
    document.body.className = '';
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

  it('launches the wizard from the first-run page via ModalTrigger/ModalStack', async () => {
    render(
      <ThemeProvider>
        <ModalProvider>
          <FirstRunPage />
        </ModalProvider>
      </ThemeProvider>
    );
    expect(screen.getByText('Start setup')).toBeTruthy();
    // Before launch there is no modal dialog.
    expect(screen.queryByRole('dialog')).toBeNull();
    fireEvent.click(screen.getByText('Start setup'));
    // After launch the SRCL Dialog (the wizard) is on screen.
    await waitFor(() => expect(screen.getByRole('dialog')).toBeTruthy());
    expect(screen.getByText('Welcome')).toBeTruthy();
  });

  it('walks the steps and POSTs the library on finish', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse({ id: 'lib1' }));
    const client = new CellarrClient({ fetchImpl });
    render(
      <ModalProvider>
        <WizardModal client={client} />
      </ModalProvider>
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
      ([url, opts]) => String(url).endsWith('/libraries') && opts?.method === 'POST'
    );
    expect(libCall).toBeTruthy();
    const body = JSON.parse((libCall![1] as RequestInit).body as string);
    expect(body.name).toBe('Films');
    expect(body.root_folders).toEqual(['/m/films']);
  });

  it('also creates an indexer when a host is provided', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse({ id: 'x' }));
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
        ([url, opts]) => String(url).endsWith('/indexers') && opts?.method === 'POST'
      );
      expect(idxCall).toBeTruthy();
      const body = JSON.parse((idxCall![1] as RequestInit).body as string);
      expect(body.host).toBe('http://idx:9117');
    });
  });
});
