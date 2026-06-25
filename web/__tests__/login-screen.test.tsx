import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

const replaceMock = vi.fn();
const refreshMock = vi.fn();
let searchParams = new URLSearchParams();

vi.mock('next/navigation', () => ({
  useRouter: () => ({ replace: replaceMock, refresh: refreshMock, push: vi.fn() }),
  useSearchParams: () => searchParams,
}));

// Keep ApiError real (the page branches on its `code`); stub the default api so
// `api.login` is controllable per test.
const loginMock = vi.fn();
vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: { login: (...args: unknown[]) => loginMock(...args) },
  };
});

import { ApiError } from '@lib/api/client';
import LoginPage from '@app/login/page';

describe('Login screen', () => {
  beforeEach(() => {
    searchParams = new URLSearchParams();
    replaceMock.mockReset();
    refreshMock.mockReset();
    loginMock.mockReset();
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

  it('POSTs the credentials and routes into the app on success', async () => {
    loginMock.mockResolvedValue({ method: 'forms', configured: true, enforced: true });
    render(<LoginPage />);

    fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'admin' } });
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'pw' } });
    fireEvent.click(screen.getByText('Log In'));

    await waitFor(() => {
      expect(loginMock).toHaveBeenCalledWith({ username: 'admin', password: 'pw' });
      expect(replaceMock).toHaveBeenCalledWith('/');
    });
  });

  it('honours a same-origin ?next= redirect target', async () => {
    searchParams = new URLSearchParams('next=/settings');
    loginMock.mockResolvedValue({ method: 'forms', configured: true, enforced: true });
    render(<LoginPage />);

    fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'admin' } });
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'pw' } });
    fireEvent.click(screen.getByText('Log In'));

    await waitFor(() => expect(replaceMock).toHaveBeenCalledWith('/settings'));
  });

  it('ignores a cross-origin ?next= and falls back to /', async () => {
    searchParams = new URLSearchParams('next=//evil.example.com');
    loginMock.mockResolvedValue({ method: 'forms', configured: true, enforced: true });
    render(<LoginPage />);

    fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'admin' } });
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'pw' } });
    fireEvent.click(screen.getByText('Log In'));

    await waitFor(() => expect(replaceMock).toHaveBeenCalledWith('/'));
  });

  it('shows an inline error and does not navigate on bad credentials', async () => {
    loginMock.mockRejectedValue(new ApiError('unauthorized', 'bad creds', 401));
    render(<LoginPage />);

    fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'admin' } });
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'nope' } });
    fireEvent.click(screen.getByText('Log In'));

    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toContain('Incorrect username or password')
    );
    expect(replaceMock).not.toHaveBeenCalled();
  });

  it('validates that both fields are present before posting', async () => {
    render(<LoginPage />);
    fireEvent.click(screen.getByText('Log In'));

    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toContain('Enter both a username and a password')
    );
    expect(loginMock).not.toHaveBeenCalled();
  });
});
