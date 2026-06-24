// App-facing entry point for the shared, typed cellarr API client.
//
// The implementation lives at `@lib/api/client` (+ `@lib/api/types`) so the SRCL
// component layer and tests can share it without importing app code. This module
// is the convenience surface for screens: import the default same-origin `api`,
// construct your own `CellarrClient`, branch on `ApiError.code`, and use
// `api.openStream(...)` / `api.poll(...)` for LIVE views.
//
// Both `@app/_lib/client` and `@lib/api/client` resolve to the same singletons.
// Screen agents consume these methods/types — they should not need to edit the
// client; every surface the five screens read is already modelled there.

export {
  ApiError,
  CellarrClient,
  api,
  resolveBaseUrl,
} from '@lib/api/client';

export type {
  ApiVersion,
  ClientOptions,
  PollHandle,
  PollOptions,
  RequestOptions,
  StreamHandle,
  StreamOptions,
} from '@lib/api/client';

export type * from '@lib/api/types';
