'use client';

// Client-side auth gate used by the AppShell. Pure data glue around the API
// client (no UI primitive), so the SRCL-only rule allows it.
//
// On mount it reads `GET /api/v1/auth/config`:
//   * a 401 `unauthorized` means a Forms session is required but missing → the
//     shell should send the user to /login (server already 303s HTML nav, but a
//     client-side push covers the SPA case where no full navigation occurred);
//   * a successful AuthStatus tells the shell whether to show a Log Out control
//     and under which method.
//
// The hook never throws; transport errors leave the gate in its loading state
// (the daemon may simply be starting) rather than locking the user out.

import * as React from 'react';

import { api, ApiError, CellarrClient } from '@lib/api/client';
import type { AuthStatus } from '@lib/api/types';

export interface AuthGate {
  /** The fetched status, or undefined while loading / on transport error. */
  status: AuthStatus | undefined;
  /** True until the first config fetch resolves (success or 401). */
  loading: boolean;
  /**
   * True when a Forms session is required but missing (config returned 401).
   * The shell uses this to redirect to /login.
   */
  unauthenticated: boolean;
  /** Log out and resolve once the session cookie has been cleared. */
  logout: () => Promise<void>;
}

export function useAuthGate(client: CellarrClient = api): AuthGate {
  const [status, setStatus] = React.useState<AuthStatus | undefined>(undefined);
  const [loading, setLoading] = React.useState(true);
  const [unauthenticated, setUnauthenticated] = React.useState(false);

  React.useEffect(() => {
    // The gate is best-effort: if the injected client doesn't expose the auth
    // surface (e.g. a partial test stub, or an older daemon), behave as if there
    // is no gate rather than crashing the whole shell.
    if (typeof client.getAuthConfig !== 'function') {
      setLoading(false);
      return;
    }
    const controller = new AbortController();
    let active = true;
    client
      .getAuthConfig(controller.signal)
      .then((s) => {
        if (!active) return;
        setStatus(s);
        setUnauthenticated(false);
      })
      .catch((err: unknown) => {
        if (!active) return;
        if (err instanceof ApiError) {
          if (err.code === 'network_error' && controller.signal.aborted) return;
          // A 401 under Forms = session required but missing.
          if (err.status === 401 || err.code === 'unauthorized') {
            setUnauthenticated(true);
            return;
          }
        }
        // Any other failure: stay quiet, leave status undefined (no gate UI).
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
      controller.abort();
    };
  }, [client]);

  const logout = React.useCallback(async () => {
    try {
      await client.logout();
    } finally {
      setStatus((prev) => (prev ? { ...prev, enforced: prev.method !== 'none' } : prev));
      setUnauthenticated(true);
    }
  }, [client]);

  return { status, loading, unauthenticated, logout };
}
