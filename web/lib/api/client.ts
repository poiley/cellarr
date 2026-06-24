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
  BlocklistRecord,
  CommandAccepted,
  CommandInfo,
  CommandResource,
  ContentNode,
  ContentRef,
  CustomFormat,
  CustomFormatV3,
  DecisionLogRecord,
  DomainEvent,
  DomainEventType,
  DownloadClientConfig,
  DownloadClientConfigV3,
  Episode,
  HealthCheck,
  HistoryRecord,
  HistoryRecordV3,
  IndexerConfig,
  IndexerConfigV3,
  Library,
  LookupCandidate,
  MediaFile,
  Movie,
  NotificationConfigV3,
  NotificationSchema,
  Page,
  QualityDefinition,
  QualityProfile,
  QueueEntry,
  QueueRecord,
  RemotePathMapping,
  RootFolder,
  Series,
  SystemStatus,
  SystemStatusV3,
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
  // Queue / activity, history, blocklist (Radarr-compatible /api/v3, paged)
  // =========================================================================

  /** The activity queue (`/api/v3/queue` → paged envelope). */
  getQueueV3(signal?: AbortSignal) {
    return this.requestV3<Page<QueueRecord>>('/queue', { signal });
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
