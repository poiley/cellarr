import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

let pathname = '/settings';

vi.mock('next/navigation', () => ({
  usePathname: () => pathname,
}));

// The shell performs a FULL navigation (window.location.assign) for the auth
// gate so the daemon re-runs its server-side gate with the current cookie.
const assignMock = vi.fn();

// Stub the default api's auth surface; the shell's useAuthGate calls these.
const getAuthConfig = vi.fn();
const logout = vi.fn();
vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      getAuthConfig: (...a: unknown[]) => getAuthConfig(...a),
      logout: (...a: unknown[]) => logout(...a),
    },
  };
});

import { ApiError } from '@lib/api/client';
import { ThemeProvider } from '@lib/ThemeProvider';
import { HotkeysProvider } from '@modules/hotkeys';
import { ModalProvider } from '@components/page/ModalContext';
import { ToastProvider } from '@app/_lib/ToastProvider';
import { CommandPaletteProvider } from '@app/_components/CommandPaletteProvider';
import AppShell from '@app/_components/AppShell';

function renderShell() {
  return render(
    <ThemeProvider>
      <HotkeysProvider>
        <ModalProvider>
          <ToastProvider>
            <CommandPaletteProvider>
              <AppShell>
                <div>shell-content</div>
              </AppShell>
            </CommandPaletteProvider>
          </ToastProvider>
        </ModalProvider>
      </HotkeysProvider>
    </ThemeProvider>
  );
}

describe('AppShell auth gate', () => {
  beforeEach(() => {
    pathname = '/settings';
    assignMock.mockReset();
    getAuthConfig.mockReset();
    logout.mockReset();
    // jsdom's location.assign is a no-op throwing stub; replace it so the shell's
    // full-navigation redirect is observable. Keep pathname at the test default.
    Object.defineProperty(window, 'location', {
      configurable: true,
      value: { ...window.location, pathname: '/settings', assign: assignMock },
    });
    window.matchMedia = vi.fn().mockReturnValue({
      matches: false,
      addEventListener: () => {},
      removeEventListener: () => {},
    }) as never;
  });
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it('redirects to /login (carrying ?next=) when /api/v1 returns 401 under Forms', async () => {
    getAuthConfig.mockRejectedValue(new ApiError('unauthorized', 'login required', 401));
    renderShell();

    await waitFor(() =>
      expect(assignMock).toHaveBeenCalledWith('/login?next=%2Fsettings')
    );
  });

  it('shows a Log Out control when Forms auth is enforced', async () => {
    getAuthConfig.mockResolvedValue({
      method: 'forms',
      configured: true,
      enforced: true,
      username: 'admin',
    });
    renderShell();

    await waitFor(() => expect(screen.getByLabelText('Log out')).toBeTruthy());
    expect(assignMock).not.toHaveBeenCalled();
  });

  it('does NOT show Log Out under None (no gate)', async () => {
    getAuthConfig.mockResolvedValue({
      method: 'none',
      configured: false,
      enforced: false,
    });
    renderShell();

    await waitFor(() => expect(screen.getByText('shell-content')).toBeTruthy());
    expect(screen.queryByLabelText('Log out')).toBeNull();
    expect(assignMock).not.toHaveBeenCalled();
  });

  it('omits Log Out under Basic (browser-managed credentials)', async () => {
    getAuthConfig.mockResolvedValue({
      method: 'basic',
      configured: true,
      enforced: true,
      username: 'admin',
    });
    renderShell();

    await waitFor(() => expect(screen.getByText('shell-content')).toBeTruthy());
    expect(screen.queryByLabelText('Log out')).toBeNull();
  });

  it('logs out and returns to /login when Log Out is clicked', async () => {
    getAuthConfig.mockResolvedValue({
      method: 'forms',
      configured: true,
      enforced: true,
      username: 'admin',
    });
    logout.mockResolvedValue({ ok: true });
    renderShell();

    const btn = await screen.findByLabelText('Log out');
    fireEvent.click(btn);

    await waitFor(() => {
      expect(logout).toHaveBeenCalled();
      expect(assignMock).toHaveBeenCalledWith('/login');
    });
  });
});
