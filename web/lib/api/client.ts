// Typed client for the cellarr daemon (docs/09-api.md).
//
// The daemon serves two HTTP surfaces from the same origin:
//   * `/api/v1` — the native, snake_case surface (crates/cellarr-api/src/native.rs);
//   * `/api/v3` — the Radarr/Sonarr-compatible shim (crates/cellarr-api/src/shim.rs),
//     which is where the seeded library data actually lives (movies, series,
//     quality profiles, custom formats, indexers, download clients, root folders,
//     remote path mappings, queue, history, blocklist, commands).
//
// Same-origin by default (the daemon serves this UI), overridable via
// NEXT_PUBLIC_API_BASE for split dev / mock-server setups. The fetch wrapper
// surfaces the daemon's structured `{ code, message }` errors as ApiError so
// callers branch on `code`, not the HTTP status.
//
// Screen agents CONSUME this client; they should not need to edit it. Every
// surface the five screens read is covered by a typed method below. Use the
// generic `request` / `requestV3` escape hatches only for routes not yet modelled.

import type {
  ApiErrorBody,
  AuthCredentialBody,
  AuthStatus,
  BackupRecord,
  BackupRestoreResult,
  BlocklistRecord,
  CommandAccepted,
  CommandInfo,
  CommandResource,
  Collection,
  CollectionUpdateBody,
  ContentNode,
  ContentRef,
  CustomFormat,
  CustomFormatSchema,
  CustomFormatTestBody,
  CustomFormatTestResult,
  CustomFormatV3,
  DecisionLogRecord,
  DelayProfile,
  DelayProfileBody,
  DomainEvent,
  DomainEventType,
  DownloadClientConfig,
  DownloadClientConfigV3,
  Episode,
  HealthCheck,
  HistoryRecord,
  HistoryRecordV3,
  ImportListBodyV3,
  ImportListConfigV3,
  ImportListExclusionBodyV3,
  ImportListExclusionV3,
  ImportListSchema,
  ImportListSyncResult,
  IndexerConfig,
  IndexerConfigV3,
  Library,
  LogFile,
  LookupCandidate,
  MediaFile,
  MediaManagement,
  Movie,
  NamingConfig,
  NamingConfigBody,
  NamingPreview,
  NamingPreviewBody,
  NamingTokens,
  NotificationConfigV3,
  NotificationSchema,
  Page,
  QualityDefinition,
  QualityDefinitionBody,
  QualityProfile,
  ReleaseProfile,
  ReleaseProfileBody,
  ReleaseProfileSchema,
  QueueEntry,
  QueueGrabResult,
  QueueRecord,
  QueueRemoveResult,
  RemotePathMapping,
  RootFolder,
  Series,
  Subtitle,
  SystemStatus,
  SystemStatusV3,
  Tag,
  TagBody,
  WantedRecord,
} from '@lib/api/types';

/** An error carrying the daemon's stable machine-readable `code`. */
export class ApiError extends Error {
  readonly code: string;
  readonly status: number;

  constructor(code: string, message: string, status: number) {
    super(message);
    this.name = 'ApiError';
    this.code = code;
    this.status = status;
  }
}

/** Resolve the API base: explicit env override, else same-origin (''). */
export function resolveBaseUrl(): string {
  const fromEnv =
    typeof process !== 'undefined' ? process.env.NEXT_PUBLIC_API_BASE : undefined;
  return (fromEnv ?? '').replace(/\/+$/, '');
}

export interface ClientOptions {
  baseUrl?: string;
  apiKey?: string;
  /** Injectable for tests; defaults to global fetch. */
  fetchImpl?: typeof fetch;
}

export type ApiVersion = 'v1' | 'v3';

export interface RequestOptions {
  method?: string;
  body?: unknown;
  query?: Record<string, string | number | boolean | undefined>;
  signal?: AbortSignal;
  /** Which API surface to target. Defaults to `v1`. */
  version?: ApiVersion;
}

function buildQuery(query?: RequestOptions['query']): string {
  if (!query) return '';
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) params.set(key, String(value));
  }
  const s = params.toString();
  return s ? `?${s}` : '';
}

/** Options for opening the live event stream. */
export interface StreamOptions {
  /** Called for every domain event, regardless of `type`. */
  onEvent?: (event: DomainEvent) => void;
  /** Per-type listeners; the key is the event's `type`/SSE `event:` name. */
  on?: Partial<Record<DomainEventType, (event: DomainEvent) => void>>;
  /** Called on a transport error (the browser will auto-reconnect). */
  onError?: (error: Event) => void;
  /** Called once the stream is open. */
  onOpen?: () => void;
}

/** A handle to a live stream; call `close()` to disconnect. */
export interface StreamHandle {
  close: () => void;
}

/** Options for a generic polling loop (for views without a push channel). */
export interface PollOptions<T> {
  /** Poll interval in milliseconds (default 5000). */
  intervalMs?: number;
  /** Whether to fire one fetch immediately before the first interval (default true). */
  immediate?: boolean;
  onData: (data: T) => void;
  onError?: (error: ApiError) => void;
}

/** A handle to a poll loop; call `stop()` to cancel. */
export interface PollHandle {
  stop: () => void;
}

export class CellarrClient {
  private readonly baseUrl: string;
  private readonly apiKey?: string;
  private readonly fetchImpl: typeof fetch;

  constructor(options: ClientOptions = {}) {
    this.baseUrl = (options.baseUrl ?? resolveBaseUrl()).replace(/\/+$/, '');
    this.apiKey = options.apiKey;
    this.fetchImpl = options.fetchImpl ?? globalThis.fetch.bind(globalThis);
  }

  /** Core fetch wrapper: JSON in/out, structured-error surfacing. */
  async request<T>(path: string, options: RequestOptions = {}): Promise<T> {
    const version = options.version ?? 'v1';
    const url = `${this.baseUrl}/api/${version}${path}${buildQuery(options.query)}`;
    const headers: Record<string, string> = { Accept: 'application/json' };
    if (options.body !== undefined) headers['Content-Type'] = 'application/json';
    if (this.apiKey) headers['X-Api-Key'] = this.apiKey;

    let res: Response;
    try {
      res = await this.fetchImpl(url, {
        method: options.method ?? 'GET',
        headers,
        // Send/store the session cookie so the auth gate sees it on `/api/v1`.
        credentials: 'same-origin',
        body: options.body !== undefined ? JSON.stringify(options.body) : undefined,
        signal: options.signal,
      });
    } catch (cause) {
      // Network / abort failures never reach the daemon's structured body.
      throw new ApiError(
        'network_error',
        cause instanceof Error ? cause.message : 'network request failed',
        0
      );
    }

    if (!res.ok) {
      throw await this.toApiError(res);
    }

    if (res.status === 204) return undefined as T;
    const text = await res.text();
    if (!text) return undefined as T;
    return JSON.parse(text) as T;
  }

  /** Convenience wrapper targeting the `/api/v3` shim. */
  requestV3<T>(path: string, options: Omit<RequestOptions, 'version'> = {}): Promise<T> {
    return this.request<T>(path, { ...options, version: 'v3' });
  }

  private async toApiError(res: Response): Promise<ApiError> {
    let code = 'http_error';
    let message = `request failed with status ${res.status}`;
    try {
      const body = (await res.json()) as Partial<ApiErrorBody>;
      if (body && typeof body.code === 'string') code = body.code;
      if (body && typeof body.message === 'string') message = body.message;
    } catch {
      // Non-JSON error body — keep the status-derived defaults.
    }
    return new ApiError(code, message, res.status);
  }

  // =========================================================================
  // System status / health
  // =========================================================================

  /** Native compact status (`/api/v1/system/status`). */
  systemStatus(signal?: AbortSignal) {
    return this.request<SystemStatus>('/system/status', { signal });
  }

  /** Radarr-compatible detailed status (`/api/v3/system/status`). */
  systemStatusV3(signal?: AbortSignal) {
    return this.requestV3<SystemStatusV3>('/system/status', { signal });
  }

  /** Health check entries (`/api/v3/health`). */
  health(signal?: AbortSignal) {
    return this.requestV3<HealthCheck[]>('/health', { signal });
  }

  // =========================================================================
  // Authentication (single admin user; gates the UI + `/api/v1`, never v3)
  // =========================================================================

  /**
   * POST a bare (non-`/api/{version}`) JSON route — used by `/login` and
   * `/logout`, which the daemon serves at the root, outside the v1/v3 prefix.
   * Sends/stores cookies (`credentials: 'same-origin'`) so the server can set
   * the `cellarr_session` cookie on login and clear it on logout.
   */
  private async requestBare<T>(
    path: string,
    options: { method?: string; body?: unknown; signal?: AbortSignal } = {}
  ): Promise<T> {
    const url = `${this.baseUrl}${path}`;
    const headers: Record<string, string> = { Accept: 'application/json' };
    if (options.body !== undefined) headers['Content-Type'] = 'application/json';
    if (this.apiKey) headers['X-Api-Key'] = this.apiKey;

    let res: Response;
    try {
      res = await this.fetchImpl(url, {
        method: options.method ?? 'GET',
        headers,
        credentials: 'same-origin',
        body: options.body !== undefined ? JSON.stringify(options.body) : undefined,
        signal: options.signal,
      });
    } catch (cause) {
      throw new ApiError(
        'network_error',
        cause instanceof Error ? cause.message : 'network request failed',
        0
      );
    }

    if (!res.ok) throw await this.toApiError(res);
    if (res.status === 204) return undefined as T;
    const text = await res.text();
    if (!text) return undefined as T;
    return JSON.parse(text) as T;
  }

  /**
   * Authenticate the admin user (`POST /login`). On success the daemon sets the
   * HttpOnly `cellarr_session` cookie and returns the new {@link AuthStatus};
   * wrong credentials reject with a 401 `unauthorized` ApiError (no cookie).
   * Only meaningful under the Forms method.
   */
  login(credential: AuthCredentialBody, signal?: AbortSignal) {
    return this.requestBare<AuthStatus>('/login', {
      method: 'POST',
      body: credential,
      signal,
    });
  }

  /**
   * End the current session (`POST /logout`). Idempotent — the daemon clears the
   * session cookie and returns `{ ok: true }` whether or not one was present.
   */
  logout(signal?: AbortSignal) {
    return this.requestBare<{ ok: boolean }>('/logout', { method: 'POST', signal });
  }

  /** The current auth-gate state (`GET /api/v1/auth/config`); never the hash. */
  getAuthConfig(signal?: AbortSignal) {
    return this.request<AuthStatus>('/auth/config', { signal });
  }

  /**
   * Switch the authentication method (`PUT /api/v1/auth/config`). Revokes all
   * sessions, so the caller (and every other client) will need to re-auth under
   * the new method.
   */
  setAuthMethod(method: AuthStatus['method'], signal?: AbortSignal) {
    return this.request<AuthStatus>('/auth/config', {
      method: 'PUT',
      body: { method },
      signal,
    });
  }

  /**
   * Set the single admin credential (`POST /api/v1/auth/credential`). The
   * password is Argon2id-hashed server-side and never echoed back. Empty
   * username/password reject with a 400 `bad_request`; success revokes sessions.
   */
  setCredential(credential: AuthCredentialBody, signal?: AbortSignal) {
    return this.request<AuthStatus>('/auth/credential', {
      method: 'POST',
      body: credential,
      signal,
    });
  }

  // =========================================================================
  // Backups (`/api/v3/system/backup`)
  // =========================================================================

  /** List backup bundles (`GET /api/v3/system/backup`). */
  listBackups(signal?: AbortSignal) {
    return this.requestV3<BackupRecord[]>('/system/backup', { signal });
  }

  /** Take a manual backup now (`POST /api/v3/system/backup`). */
  createBackup(signal?: AbortSignal) {
    return this.requestV3<BackupRecord>('/system/backup', { method: 'POST', signal });
  }

  /** Delete a backup bundle (`DELETE /api/v3/system/backup/{id}`, idempotent). */
  deleteBackup(id: number | string, signal?: AbortSignal) {
    return this.requestV3<void>(`/system/backup/${id}`, { method: 'DELETE', signal });
  }

  /**
   * The absolute URL of a backup bundle's raw bytes
   * (`GET /api/v3/system/backup/{id}`, served as an attachment). Used as an
   * anchor href so the browser performs the download natively.
   */
  backupDownloadUrl(id: number | string): string {
    return `${this.baseUrl}/api/v3/system/backup/${id}`;
  }

  /**
   * Restore a stored backup (`POST /api/v3/system/backup/restore/{id}`).
   * DESTRUCTIVE: replaces the live database. The daemon takes an automatic
   * pre-restore safety backup first and reports it in the result.
   */
  restoreBackup(id: number | string, signal?: AbortSignal) {
    return this.requestV3<BackupRestoreResult>(`/system/backup/restore/${id}`, {
      method: 'POST',
      signal,
    });
  }

  // =========================================================================
  // Log files (`/api/v3/log/file`)
  // =========================================================================

  /** List available log files (`GET /api/v3/log/file`). */
  listLogFiles(signal?: AbortSignal) {
    return this.requestV3<LogFile[]>('/log/file', { signal });
  }

  /**
   * Fetch the tail of a log file (`GET /api/v3/log/file/{name}`), returned as
   * plain text. `lines` bounds how many trailing lines to return (default 500,
   * max 10000 server-side).
   */
  async getLogFile(name: string, lines?: number, signal?: AbortSignal): Promise<string> {
    const url =
      `${this.baseUrl}/api/v3/log/file/${encodeURIComponent(name)}` +
      (lines ? `?lines=${lines}` : '');
    const headers: Record<string, string> = { Accept: 'text/plain' };
    if (this.apiKey) headers['X-Api-Key'] = this.apiKey;

    let res: Response;
    try {
      res = await this.fetchImpl(url, { method: 'GET', headers, signal });
    } catch (cause) {
      throw new ApiError(
        'network_error',
        cause instanceof Error ? cause.message : 'network request failed',
        0
      );
    }
    if (!res.ok) throw await this.toApiError(res);
    return res.text();
  }

  // =========================================================================
  // Libraries + content (native /api/v1)
  // =========================================================================

  listLibraries(signal?: AbortSignal) {
    return this.request<Library[]>('/libraries', { signal });
  }

  getLibrary(id: string, signal?: AbortSignal) {
    return this.request<Library>(`/libraries/${id}`, { signal });
  }

  listContent(libraryId: string, signal?: AbortSignal) {
    return this.request<ContentRef[]>(`/libraries/${libraryId}/content`, { signal });
  }

  getContent(id: string, signal?: AbortSignal) {
    return this.request<ContentNode>(`/content/${id}`, { signal });
  }

  listContentFiles(id: string, signal?: AbortSignal) {
    return this.request<MediaFile[]>(`/content/${id}/files`, { signal });
  }

  /** Subtitles fetched for a content node (`/api/v1/content/{id}/subtitles`). */
  listContentSubtitles(id: string, signal?: AbortSignal) {
    return this.request<Subtitle[]>(`/content/${id}/subtitles`, { signal });
  }

  contentHistory(id: string, signal?: AbortSignal) {
    return this.request<HistoryRecord[]>(`/content/${id}/history`, { signal });
  }

  // =========================================================================
  // Movies / series / episodes (Radarr/Sonarr-compatible /api/v3)
  // =========================================================================

  listMovies(signal?: AbortSignal) {
    return this.requestV3<Movie[]>('/movie', { signal });
  }

  listSeries(signal?: AbortSignal) {
    return this.requestV3<Series[]>('/series', { signal });
  }

  /** Episodes for a series (`/api/v3/episode?seriesId=…`). */
  listEpisodes(seriesId: string, signal?: AbortSignal) {
    return this.requestV3<Episode[]>('/episode', {
      query: { seriesId },
      signal,
    });
  }

  /**
   * Delete a movie (`DELETE /api/v3/movie/{id}`). `deleteFiles` recycles/unlinks
   * its media; `addImportExclusion` records an exclusion so an import list cannot
   * re-add it. Both default off, matching the Radarr delete contract.
   */
  deleteMovie(
    id: string,
    opts: { deleteFiles?: boolean; addImportExclusion?: boolean } = {},
    signal?: AbortSignal
  ) {
    return this.requestV3<void>(`/movie/${id}`, {
      method: 'DELETE',
      query: {
        deleteFiles: opts.deleteFiles ?? false,
        addImportExclusion: opts.addImportExclusion ?? false,
      },
      signal,
    });
  }

  /**
   * Delete a series and its season/episode subtree (`DELETE /api/v3/series/{id}`).
   * Same flags as {@link deleteMovie}; mirrors the Sonarr delete contract.
   */
  deleteSeries(
    id: string,
    opts: { deleteFiles?: boolean; addImportExclusion?: boolean } = {},
    signal?: AbortSignal
  ) {
    return this.requestV3<void>(`/series/${id}`, {
      method: 'DELETE',
      query: {
        deleteFiles: opts.deleteFiles ?? false,
        addImportExclusion: opts.addImportExclusion ?? false,
      },
      signal,
    });
  }

  /** Free-text movie lookup (`/api/v3/movie/lookup?term=…`). */
  movieLookup(term: string, signal?: AbortSignal) {
    return this.requestV3<LookupCandidate[]>('/movie/lookup', {
      query: { term },
      signal,
    });
  }

  /** Free-text series lookup (`/api/v3/series/lookup?term=…`). */
  seriesLookup(term: string, signal?: AbortSignal) {
    return this.requestV3<LookupCandidate[]>('/series/lookup', {
      query: { term },
      signal,
    });
  }

  /** Monitored items that are missing an acceptable file (`/api/v3/wanted/missing`). */
  wantedMissing(signal?: AbortSignal) {
    return this.requestV3<Page<WantedRecord>>('/wanted/missing', { signal });
  }

  // =========================================================================
  // Collections (Radarr-compatible /api/v3 — TMDb collection import lists)
  // =========================================================================

  /**
   * List movie collections (`GET /api/v3/collection`). Radarr-only — the
   * Sonarr/cellarr faces return `[]`. Each entry carries its member `movies[]`
   * (we surface only the count) plus the writable `monitored` flag.
   */
  listCollections(signal?: AbortSignal) {
    return this.requestV3<Collection[]>('/collection', { signal });
  }

  /** A single collection (`GET /api/v3/collection/{id}`). */
  getCollection(id: number, signal?: AbortSignal) {
    return this.requestV3<Collection>(`/collection/${id}`, { signal });
  }

  /**
   * Persist the writable subset of a collection (`PUT /api/v3/collection/{id}`)
   * onto its backing import list. The list view sends `{ monitored }`; the body
   * also accepts `qualityProfileId`. Returns the updated collection.
   */
  updateCollection(id: number, body: CollectionUpdateBody, signal?: AbortSignal) {
    return this.requestV3<Collection>(`/collection/${id}`, {
      method: 'PUT',
      body,
      signal,
    });
  }

  // =========================================================================
  // Quality profiles + definitions (Radarr-compatible /api/v3)
  // =========================================================================

  /**
   * List quality profiles. Reads `/api/v3/qualityprofile`, where the seeded
   * HD-1080p / WEB-1080p profiles actually live — the native
   * `/api/v1/qualityprofiles` route currently returns `[]`.
   */
  getQualityProfiles(signal?: AbortSignal) {
    return this.requestV3<QualityProfile[]>('/qualityprofile', { signal });
  }

  getQualityProfile(id: string, signal?: AbortSignal) {
    return this.requestV3<QualityProfile>(`/qualityprofile/${id}`, { signal });
  }

  createQualityProfile(body: Partial<QualityProfile>, signal?: AbortSignal) {
    return this.requestV3<QualityProfile>('/qualityprofile', {
      method: 'POST',
      body,
      signal,
    });
  }

  updateQualityProfile(id: string, body: Partial<QualityProfile>, signal?: AbortSignal) {
    return this.requestV3<QualityProfile>(`/qualityprofile/${id}`, {
      method: 'PUT',
      body,
      signal,
    });
  }

  deleteQualityProfile(id: string, signal?: AbortSignal) {
    return this.requestV3<void>(`/qualityprofile/${id}`, {
      method: 'DELETE',
      signal,
    });
  }

  /** Quality definitions (sizes/weights) — `/api/v3/qualitydefinition`. */
  getQualityDefinitions(signal?: AbortSignal) {
    return this.requestV3<QualityDefinition[]>('/qualitydefinition', { signal });
  }

  /**
   * Edit a single quality definition's title / size bounds
   * (`PUT /api/v3/qualitydefinition/{id}`). `id` is the `rank + 1` from a GET
   * element; sizes are bytes-per-minute. Returns the updated definition in the
   * same shape as a GET element.
   */
  updateQualityDefinition(id: number, body: QualityDefinitionBody, signal?: AbortSignal) {
    return this.requestV3<QualityDefinition>(`/qualitydefinition/${id}`, {
      method: 'PUT',
      body,
      signal,
    });
  }

  /**
   * Bulk-edit quality definitions (`PUT /api/v3/qualitydefinition/update`). The
   * body is an array of {@link QualityDefinitionBody}, each REQUIRING its own
   * `id`. Returns the array of updated definitions.
   */
  updateQualityDefinitions(bodies: QualityDefinitionBody[], signal?: AbortSignal) {
    return this.requestV3<QualityDefinition[]>('/qualitydefinition/update', {
      method: 'PUT',
      body: bodies,
      signal,
    });
  }

  // =========================================================================
  // Custom formats (Radarr-compatible /api/v3)
  // =========================================================================

  /** Custom formats in the native snake_case shape (`/api/v1/customformats`). */
  listCustomFormats(signal?: AbortSignal) {
    return this.request<CustomFormat[]>('/customformats', { signal });
  }

  /** Custom formats in the Radarr-compatible shape (`/api/v3/customformat`). */
  listCustomFormatsV3(signal?: AbortSignal) {
    return this.requestV3<CustomFormatV3[]>('/customformat', { signal });
  }

  /** A single v3 custom format (`/api/v3/customformat/{id}`). */
  getCustomFormatV3(id: number, signal?: AbortSignal) {
    return this.requestV3<CustomFormatV3>(`/customformat/${id}`, { signal });
  }

  /**
   * The catalogue of specification templates a custom format is built from
   * (`/api/v3/customformat/schema`) — drives the editor's per-implementation
   * fields (a `value` textbox/select, or Size's `min`/`max` numbers).
   */
  getCustomFormatSchema(signal?: AbortSignal) {
    return this.requestV3<CustomFormatSchema[]>('/customformat/schema', { signal });
  }

  /**
   * Report which stored custom formats match a release title, for the editor's
   * live preview (`POST /api/v3/customformat/test`).
   */
  testCustomFormat(body: CustomFormatTestBody, signal?: AbortSignal) {
    return this.requestV3<CustomFormatTestResult[]>('/customformat/test', {
      method: 'POST',
      body,
      signal,
    });
  }

  createCustomFormat(body: Partial<CustomFormatV3>, signal?: AbortSignal) {
    return this.requestV3<CustomFormatV3>('/customformat', {
      method: 'POST',
      body,
      signal,
    });
  }

  updateCustomFormat(id: number, body: Partial<CustomFormatV3>, signal?: AbortSignal) {
    return this.requestV3<CustomFormatV3>(`/customformat/${id}`, {
      method: 'PUT',
      body,
      signal,
    });
  }

  deleteCustomFormat(id: number, signal?: AbortSignal) {
    return this.requestV3<void>(`/customformat/${id}`, {
      method: 'DELETE',
      signal,
    });
  }

  // =========================================================================
  // Delay profiles (Radarr/Sonarr-compatible /api/v3)
  // =========================================================================

  /** Delay profiles, ordered by `order` (`GET /api/v3/delayprofile`). */
  getDelayProfiles(signal?: AbortSignal) {
    return this.requestV3<DelayProfile[]>('/delayprofile', { signal });
  }

  /** A single delay profile (`GET /api/v3/delayprofile/{id}`). */
  getDelayProfile(id: number, signal?: AbortSignal) {
    return this.requestV3<DelayProfile>(`/delayprofile/${id}`, { signal });
  }

  createDelayProfile(body: DelayProfileBody, signal?: AbortSignal) {
    return this.requestV3<DelayProfile>('/delayprofile', {
      method: 'POST',
      body,
      signal,
    });
  }

  updateDelayProfile(id: number, body: DelayProfileBody, signal?: AbortSignal) {
    return this.requestV3<DelayProfile>(`/delayprofile/${id}`, {
      method: 'PUT',
      body,
      signal,
    });
  }

  deleteDelayProfile(id: number, signal?: AbortSignal) {
    return this.requestV3<void>(`/delayprofile/${id}`, { method: 'DELETE', signal });
  }

  // =========================================================================
  // Release profiles (Sonarr-compatible /api/v3)
  // =========================================================================

  /** Release profiles (`GET /api/v3/releaseprofile`). */
  getReleaseProfiles(signal?: AbortSignal) {
    return this.requestV3<ReleaseProfile[]>('/releaseprofile', { signal });
  }

  /** A single release profile (`GET /api/v3/releaseprofile/{id}`). */
  getReleaseProfile(id: number, signal?: AbortSignal) {
    return this.requestV3<ReleaseProfile>(`/releaseprofile/${id}`, { signal });
  }

  /**
   * The editor template for a release profile
   * (`GET /api/v3/releaseprofile/schema`) — the blank object shape plus the
   * `fields[]` describing each editable input.
   */
  getReleaseProfileSchema(signal?: AbortSignal) {
    return this.requestV3<ReleaseProfileSchema>('/releaseprofile/schema', { signal });
  }

  createReleaseProfile(body: ReleaseProfileBody, signal?: AbortSignal) {
    return this.requestV3<ReleaseProfile>('/releaseprofile', {
      method: 'POST',
      body,
      signal,
    });
  }

  updateReleaseProfile(id: number, body: ReleaseProfileBody, signal?: AbortSignal) {
    return this.requestV3<ReleaseProfile>(`/releaseprofile/${id}`, {
      method: 'PUT',
      body,
      signal,
    });
  }

  deleteReleaseProfile(id: number, signal?: AbortSignal) {
    return this.requestV3<void>(`/releaseprofile/${id}`, { method: 'DELETE', signal });
  }

  // =========================================================================
  // Indexers (Radarr-compatible /api/v3)
  // =========================================================================

  /** Indexers in the native snake_case shape (`/api/v1/indexers`). */
  listIndexers(signal?: AbortSignal) {
    return this.request<IndexerConfig[]>('/indexers', { signal });
  }

  /** Indexers in the Radarr-compatible shape (`/api/v3/indexer`). */
  listIndexersV3(signal?: AbortSignal) {
    return this.requestV3<IndexerConfigV3[]>('/indexer', { signal });
  }

  createIndexer(body: Partial<IndexerConfigV3>, signal?: AbortSignal) {
    return this.requestV3<IndexerConfigV3>('/indexer', { method: 'POST', body, signal });
  }

  updateIndexer(id: number, body: Partial<IndexerConfigV3>, signal?: AbortSignal) {
    return this.requestV3<IndexerConfigV3>(`/indexer/${id}`, {
      method: 'PUT',
      body,
      signal,
    });
  }

  deleteIndexer(id: number, signal?: AbortSignal) {
    return this.requestV3<void>(`/indexer/${id}`, { method: 'DELETE', signal });
  }

  testIndexer(body: Partial<IndexerConfigV3>, signal?: AbortSignal) {
    return this.requestV3<unknown>('/indexer/test', { method: 'POST', body, signal });
  }

  // =========================================================================
  // Download clients (Radarr-compatible /api/v3)
  // =========================================================================

  /** Download clients in the native snake_case shape (`/api/v1/downloadclients`). */
  listDownloadClients(signal?: AbortSignal) {
    return this.request<DownloadClientConfig[]>('/downloadclients', { signal });
  }

  /** Download clients in the Radarr-compatible shape (`/api/v3/downloadclient`). */
  listDownloadClientsV3(signal?: AbortSignal) {
    return this.requestV3<DownloadClientConfigV3[]>('/downloadclient', { signal });
  }

  createDownloadClient(body: Partial<DownloadClientConfigV3>, signal?: AbortSignal) {
    return this.requestV3<DownloadClientConfigV3>('/downloadclient', {
      method: 'POST',
      body,
      signal,
    });
  }

  updateDownloadClient(id: number, body: Partial<DownloadClientConfigV3>, signal?: AbortSignal) {
    return this.requestV3<DownloadClientConfigV3>(`/downloadclient/${id}`, {
      method: 'PUT',
      body,
      signal,
    });
  }

  deleteDownloadClient(id: number, signal?: AbortSignal) {
    return this.requestV3<void>(`/downloadclient/${id}`, { method: 'DELETE', signal });
  }

  testDownloadClient(body: Partial<DownloadClientConfigV3>, signal?: AbortSignal) {
    return this.requestV3<unknown>('/downloadclient/test', {
      method: 'POST',
      body,
      signal,
    });
  }

  // =========================================================================
  // Notifications (Radarr/Sonarr-compatible /api/v3)
  // =========================================================================

  /** Configured notifications (`GET /api/v3/notification`). */
  listNotifications(signal?: AbortSignal) {
    return this.requestV3<NotificationConfigV3[]>('/notification', { signal });
  }

  /** The connector templates a notification is built from (`GET /api/v3/notification/schema`). */
  getNotificationSchema(signal?: AbortSignal) {
    return this.requestV3<NotificationSchema[]>('/notification/schema', { signal });
  }

  createNotification(body: Partial<NotificationConfigV3>, signal?: AbortSignal) {
    return this.requestV3<NotificationConfigV3>('/notification', {
      method: 'POST',
      body,
      signal,
    });
  }

  updateNotification(id: number, body: Partial<NotificationConfigV3>, signal?: AbortSignal) {
    return this.requestV3<NotificationConfigV3>(`/notification/${id}`, {
      method: 'PUT',
      body,
      signal,
    });
  }

  deleteNotification(id: number, signal?: AbortSignal) {
    return this.requestV3<void>(`/notification/${id}`, { method: 'DELETE', signal });
  }

  testNotification(body: Partial<NotificationConfigV3>, signal?: AbortSignal) {
    return this.requestV3<unknown>('/notification/test', { method: 'POST', body, signal });
  }

  // =========================================================================
  // Tags (Radarr/Sonarr-compatible /api/v3/tag) — DB-backed, ids stable
  // =========================================================================

  /** Every label tag (`GET /api/v3/tag`). */
  listTags(signal?: AbortSignal) {
    return this.requestV3<Tag[]>('/tag', { signal });
  }

  /** A single tag (`GET /api/v3/tag/{id}`). */
  getTag(id: number, signal?: AbortSignal) {
    return this.requestV3<Tag>(`/tag/${id}`, { signal });
  }

  /**
   * Create a tag (`POST /api/v3/tag`). Label de-dup is case-insensitive
   * server-side, so creating an existing label returns that tag rather than a
   * duplicate.
   */
  createTag(body: TagBody, signal?: AbortSignal) {
    return this.requestV3<Tag>('/tag', { method: 'POST', body, signal });
  }

  /** Rename a tag (`PUT /api/v3/tag/{id}`). */
  updateTag(id: number, body: TagBody, signal?: AbortSignal) {
    return this.requestV3<Tag>(`/tag/${id}`, { method: 'PUT', body, signal });
  }

  /** Delete a tag (`DELETE /api/v3/tag/{id}`, idempotent). */
  deleteTag(id: number, signal?: AbortSignal) {
    return this.requestV3<void>(`/tag/${id}`, { method: 'DELETE', signal });
  }

  // =========================================================================
  // Root folders + remote path mappings (Radarr-compatible /api/v3)
  // =========================================================================

  listRootFolders(signal?: AbortSignal) {
    return this.requestV3<RootFolder[]>('/rootfolder', { signal });
  }

  listRemotePathMappings(signal?: AbortSignal) {
    return this.requestV3<RemotePathMapping[]>('/remotepathmapping', { signal });
  }

  createRemotePathMapping(body: Partial<RemotePathMapping>, signal?: AbortSignal) {
    return this.requestV3<RemotePathMapping>('/remotepathmapping', {
      method: 'POST',
      body,
      signal,
    });
  }

  updateRemotePathMapping(id: number, body: Partial<RemotePathMapping>, signal?: AbortSignal) {
    return this.requestV3<RemotePathMapping>(`/remotepathmapping/${id}`, {
      method: 'PUT',
      body,
      signal,
    });
  }

  deleteRemotePathMapping(id: number, signal?: AbortSignal) {
    return this.requestV3<void>(`/remotepathmapping/${id}`, {
      method: 'DELETE',
      signal,
    });
  }

  // =========================================================================
  // Media management — naming config (Radarr/Sonarr-compatible /api/v3/config)
  // =========================================================================

  /** The persisted per-media-type naming formats (`GET /api/v3/config/naming`). */
  getNamingConfig(signal?: AbortSignal) {
    return this.requestV3<NamingConfig>('/config/naming', { signal });
  }

  /**
   * Update one or more naming formats (`PUT /api/v3/config/naming`). The body is a
   * partial merge; an invalid/under-specified format is rejected with 400 and
   * nothing is persisted. Returns the full, updated naming config.
   */
  updateNamingConfig(body: NamingConfigBody, signal?: AbortSignal) {
    return this.requestV3<NamingConfig>('/config/naming', {
      method: 'PUT',
      body,
      signal,
    });
  }

  /** The insertable token vocabulary per target (`GET /api/v3/config/naming/tokens`). */
  getNamingTokens(signal?: AbortSignal) {
    return this.requestV3<NamingTokens>('/config/naming/tokens', { signal });
  }

  /**
   * Render a candidate format against a sample context for the live preview
   * (`POST /api/v3/config/naming/preview`). A malformed / missing-required-token
   * format → 400.
   */
  previewNaming(body: NamingPreviewBody, signal?: AbortSignal) {
    return this.requestV3<NamingPreview>('/config/naming/preview', {
      method: 'POST',
      body,
      signal,
    });
  }

  /**
   * The persisted media-management blob — naming, recycle-bin, post-commit
   * permissions, and extra-files config (`GET /api/v3/config/mediamanagement`).
   * Permissions + extra-files apply AFTER a media commit and never roll the
   * imported media back on failure.
   */
  getMediaManagement(signal?: AbortSignal) {
    return this.requestV3<MediaManagement>('/config/mediamanagement', { signal });
  }

  /** Partial-merge update of the media-management blob (`PUT /api/v3/config/mediamanagement`). */
  updateMediaManagement(body: Partial<MediaManagement>, signal?: AbortSignal) {
    return this.requestV3<MediaManagement>('/config/mediamanagement', {
      method: 'PUT',
      body,
      signal,
    });
  }

  // =========================================================================
  // Import lists + exclusions (Radarr/Sonarr-compatible /api/v3)
  // =========================================================================

  /** Configured import lists (`GET /api/v3/importlist`). */
  listImportLists(signal?: AbortSignal) {
    return this.requestV3<ImportListConfigV3[]>('/importlist', { signal });
  }

  /** The import-list source templates (`GET /api/v3/importlist/schema`). */
  getImportListSchema(signal?: AbortSignal) {
    return this.requestV3<ImportListSchema[]>('/importlist/schema', { signal });
  }

  createImportList(body: ImportListBodyV3, signal?: AbortSignal) {
    return this.requestV3<ImportListConfigV3>('/importlist', {
      method: 'POST',
      body,
      signal,
    });
  }

  updateImportList(id: number, body: ImportListBodyV3, signal?: AbortSignal) {
    return this.requestV3<ImportListConfigV3>(`/importlist/${id}`, {
      method: 'PUT',
      body,
      signal,
    });
  }

  deleteImportList(id: number, signal?: AbortSignal) {
    return this.requestV3<void>(`/importlist/${id}`, { method: 'DELETE', signal });
  }

  /** Validate an import-list body (`POST /api/v3/importlist/test`). */
  testImportList(body: ImportListBodyV3, signal?: AbortSignal) {
    return this.requestV3<unknown>('/importlist/test', { method: 'POST', body, signal });
  }

  /**
   * Trigger a safeguarded sync for one import list
   * (`POST /api/v3/importlist/{id}/sync`). A failed source fetch changes nothing
   * (the safeguard); the response reports per-list `fetchSucceeded`/added/cleaned.
   */
  syncImportList(id: number, signal?: AbortSignal) {
    return this.requestV3<ImportListSyncResult>(`/importlist/${id}/sync`, {
      method: 'POST',
      signal,
    });
  }

  /** Configured import-list exclusions (`GET /api/v3/importlistexclusion`). */
  listImportListExclusions(signal?: AbortSignal) {
    return this.requestV3<ImportListExclusionV3[]>('/importlistexclusion', { signal });
  }

  createImportListExclusion(body: ImportListExclusionBodyV3, signal?: AbortSignal) {
    return this.requestV3<ImportListExclusionV3>('/importlistexclusion', {
      method: 'POST',
      body,
      signal,
    });
  }

  deleteImportListExclusion(id: number, signal?: AbortSignal) {
    return this.requestV3<void>(`/importlistexclusion/${id}`, { method: 'DELETE', signal });
  }

  // =========================================================================
  // Queue / activity, history, blocklist (Radarr-compatible /api/v3, paged)
  // =========================================================================

  /** The activity queue (`/api/v3/queue` → paged envelope). */
  getQueueV3(signal?: AbortSignal) {
    return this.requestV3<Page<QueueRecord>>('/queue', { signal });
  }

  /**
   * Remove a queue item (`DELETE /api/v3/queue/{id}`). `removeFromClient` also
   * tells the download client to delete the download + its data; `blocklist` adds
   * the release to the blocklist so a re-search never re-grabs it. Idempotent.
   */
  removeQueueItem(
    id: string,
    opts: { removeFromClient?: boolean; blocklist?: boolean } = {},
    signal?: AbortSignal
  ) {
    return this.requestV3<QueueRemoveResult>(`/queue/${id}`, {
      method: 'DELETE',
      query: {
        removeFromClient: opts.removeFromClient ?? false,
        blocklist: opts.blocklist ?? false,
      },
      signal,
    });
  }

  /** Change a queued download's category (`PUT /api/v3/queue/{id}`). */
  updateQueueCategory(id: string, category: string, signal?: AbortSignal) {
    return this.requestV3<QueueRecord>(`/queue/${id}`, {
      method: 'PUT',
      body: { category },
      signal,
    });
  }

  /**
   * Manually import a completed-but-unmatched download from the queue
   * (`POST /api/v3/queue/grab`). The grab is identified by `id`; `contentId`
   * overrides the content node, and `path` is the on-disk download path.
   */
  grabQueueItem(
    body: { id: number | string; contentId?: string; path: string },
    signal?: AbortSignal
  ) {
    return this.requestV3<QueueGrabResult>('/queue/grab', {
      method: 'POST',
      body,
      signal,
    });
  }

  /** History records (`/api/v3/history` → paged envelope). */
  getHistoryV3(signal?: AbortSignal) {
    return this.requestV3<Page<HistoryRecordV3>>('/history', { signal });
  }

  /** Blocklist records (`/api/v3/blocklist` → paged envelope). */
  getBlocklist(signal?: AbortSignal) {
    return this.requestV3<Page<BlocklistRecord>>('/blocklist', { signal });
  }

  /** Remove a single blocklist entry (`DELETE /api/v3/blocklist/{id}`). */
  deleteBlocklistItem(id: string, signal?: AbortSignal) {
    return this.requestV3<void>(`/blocklist/${id}`, { method: 'DELETE', signal });
  }

  // =========================================================================
  // Commands — trigger searches / refreshes (both surfaces)
  // =========================================================================

  /** The native command catalogue (`/api/v1/commands`). */
  getCommands(signal?: AbortSignal) {
    return this.request<CommandInfo[]>('/commands', { signal });
  }

  /** Trigger a native command (`POST /api/v1/commands`). */
  runCommand(name: string, contentId?: string, signal?: AbortSignal) {
    return this.request<CommandAccepted>('/commands', {
      method: 'POST',
      body: { name, content_id: contentId },
      signal,
    });
  }

  /** The Radarr-compatible command list (`/api/v3/command`). */
  listCommandsV3(signal?: AbortSignal) {
    return this.requestV3<CommandResource[]>('/command', { signal });
  }

  /**
   * Trigger a Radarr-compatible command (`POST /api/v3/command`), e.g.
   * `{ name: 'MissingItemSearch' }` or `{ name: 'MoviesSearch', movieIds: […] }`.
   */
  runCommandV3(body: { name: string } & Record<string, unknown>, signal?: AbortSignal) {
    return this.requestV3<CommandResource>('/command', {
      method: 'POST',
      body,
      signal,
    });
  }

  // =========================================================================
  // Decision log (native /api/v1)
  // =========================================================================

  getDecisionLog(runId: string, signal?: AbortSignal) {
    return this.request<DecisionLogRecord[]>(`/decisionlog/${runId}`, { signal });
  }

  // =========================================================================
  // Native compact queue (legacy; the activity screen uses getQueueV3)
  // =========================================================================

  getQueue(signal?: AbortSignal) {
    return this.request<QueueEntry[]>('/queue', { signal });
  }

  getHistory(contentId?: string, signal?: AbortSignal) {
    return this.request<HistoryRecord[]>('/history', {
      query: contentId ? { content: contentId } : undefined,
      signal,
    });
  }

  // =========================================================================
  // Live updates — SSE stream + a generic poll helper
  // =========================================================================

  /** The absolute URL of the live SSE stream (`/api/v1/stream`). */
  streamUrl(): string {
    return `${this.baseUrl}/api/v1/stream`;
  }

  /**
   * Open the live event stream (`GET /api/v1/stream`, Server-Sent Events).
   * The daemon pushes `queue_progress`, `import_completed`, `decision_logged`,
   * and `command_queued` events as real domain transitions happen — there is no
   * polling timer behind it. Browser-only (uses `EventSource`); returns a handle
   * whose `close()` ends the subscription.
   *
   * Prefer this for LIVE views (activity / queue progress). For surfaces without
   * a matching push event, use {@link poll}.
   */
  openStream(options: StreamOptions = {}): StreamHandle {
    if (typeof EventSource === 'undefined') {
      // No-op handle on the server / in non-DOM environments.
      return { close: () => {} };
    }
    const source = new EventSource(this.streamUrl());

    const dispatch = (raw: string) => {
      let event: DomainEvent;
      try {
        event = JSON.parse(raw) as DomainEvent;
      } catch {
        return;
      }
      options.onEvent?.(event);
      options.on?.[event.type]?.(event);
    };

    // Named events carry the same payload tagged by `type`; the default
    // `message` listener covers servers/proxies that drop the event name.
    source.onmessage = (e) => dispatch(e.data);
    const types: DomainEventType[] = [
      'queue_progress',
      'import_completed',
      'decision_logged',
      'command_queued',
    ];
    for (const t of types) {
      source.addEventListener(t, (e) => dispatch((e as MessageEvent).data));
    }
    if (options.onOpen) source.onopen = () => options.onOpen?.();
    if (options.onError) source.onerror = (e) => options.onError?.(e);

    return { close: () => source.close() };
  }

  /**
   * Poll any fetcher on an interval — the fallback live mechanism for views the
   * SSE stream does not cover (e.g. periodic history/blocklist refresh). Returns
   * a handle whose `stop()` cancels the loop and aborts the in-flight request.
   */
  poll<T>(fetcher: (signal: AbortSignal) => Promise<T>, options: PollOptions<T>): PollHandle {
    const intervalMs = options.intervalMs ?? 5000;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const controller = new AbortController();

    const tick = async () => {
      if (stopped) return;
      try {
        const data = await fetcher(controller.signal);
        if (!stopped) options.onData(data);
      } catch (err) {
        if (!stopped && err instanceof ApiError) options.onError?.(err);
      } finally {
        if (!stopped) timer = setTimeout(tick, intervalMs);
      }
    };

    if (options.immediate ?? true) {
      void tick();
    } else {
      timer = setTimeout(tick, intervalMs);
    }

    return {
      stop: () => {
        stopped = true;
        if (timer) clearTimeout(timer);
        controller.abort();
      },
    };
  }
}

/** A default same-origin client for app code. Tests construct their own. */
export const api = new CellarrClient();
