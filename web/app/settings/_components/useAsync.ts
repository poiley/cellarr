'use client';

// Tiny async-state helper shared by the settings sections. Not a UI primitive —
// pure data glue around the API client, allowed by the SRCL-only rule.

import * as React from 'react';

import { ApiError } from '@lib/api/client';

export interface AsyncState<T> {
  data: T | undefined;
  loading: boolean;
  error: ApiError | undefined;
  reload: () => void;
}

/** Run an async loader on mount (and on demand), tracking loading/error. */
export function useAsync<T>(loader: (signal: AbortSignal) => Promise<T>): AsyncState<T> {
  const [data, setData] = React.useState<T | undefined>(undefined);
  const [loading, setLoading] = React.useState<boolean>(true);
  const [error, setError] = React.useState<ApiError | undefined>(undefined);
  const [nonce, setNonce] = React.useState(0);

  // Keep the latest loader without retriggering the effect each render.
  const loaderRef = React.useRef(loader);
  loaderRef.current = loader;

  React.useEffect(() => {
    const controller = new AbortController();
    let active = true;
    setLoading(true);
    setError(undefined);
    loaderRef
      .current(controller.signal)
      .then((result) => {
        if (active) setData(result);
      })
      .catch((err: unknown) => {
        if (!active) return;
        if (err instanceof ApiError) {
          // An aborted request is not a user-facing error.
          if (err.code === 'network_error' && controller.signal.aborted) return;
          setError(err);
        } else {
          setError(new ApiError('unknown_error', 'unexpected failure', 0));
        }
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
      controller.abort();
    };
  }, [nonce]);

  const reload = React.useCallback(() => setNonce((n) => n + 1), []);

  return { data, loading, error, reload };
}

/** Normalize any thrown value into an ApiError for consistent display. */
export function toApiError(err: unknown): ApiError {
  if (err instanceof ApiError) return err;
  if (err instanceof Error) return new ApiError('unknown_error', err.message, 0);
  return new ApiError('unknown_error', 'unexpected failure', 0);
}
