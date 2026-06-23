// Typed client for the daemon's native `/api/v1` (docs/09-api.md).
//
// Same-origin by default (the daemon serves this UI), overridable via
// NEXT_PUBLIC_API_BASE for split dev / mock-server setups. The fetch wrapper
// surfaces the daemon's structured `{ code, message }` errors as ApiError so
// callers branch on `code`, not the HTTP status.

import type {
  ApiErrorBody,
  CommandAccepted,
  CommandInfo,
  ContentNode,
  ContentRef,
  CustomFormat,
  DecisionLogRecord,
  DownloadClientConfig,
  HistoryRecord,
  IndexerConfig,
  Library,
  MediaFile,
  QualityProfile,
  QueueEntry,
  SystemStatus,
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

export interface RequestOptions {
  method?: string;
  body?: unknown;
  query?: Record<string, string | number | boolean | undefined>;
  signal?: AbortSignal;
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
    const url = `${this.baseUrl}/api/v1${path}${buildQuery(options.query)}`;
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

  // --- typed endpoint helpers ---------------------------------------------

  systemStatus(signal?: AbortSignal) {
    return this.request<SystemStatus>('/system/status', { signal });
  }

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

  listIndexers(signal?: AbortSignal) {
    return this.request<IndexerConfig[]>('/indexers', { signal });
  }

  listDownloadClients(signal?: AbortSignal) {
    return this.request<DownloadClientConfig[]>('/downloadclients', { signal });
  }

  getQualityProfiles(ids?: string[], signal?: AbortSignal) {
    return this.request<QualityProfile[]>('/qualityprofiles', {
      query: ids && ids.length ? { ids: ids.join(',') } : undefined,
      signal,
    });
  }

  getQualityProfile(id: string, signal?: AbortSignal) {
    return this.request<QualityProfile>(`/qualityprofiles/${id}`, { signal });
  }

  listCustomFormats(signal?: AbortSignal) {
    return this.request<CustomFormat[]>('/customformats', { signal });
  }

  getQueue(signal?: AbortSignal) {
    return this.request<QueueEntry[]>('/queue', { signal });
  }

  getHistory(contentId?: string, signal?: AbortSignal) {
    return this.request<HistoryRecord[]>('/history', {
      query: contentId ? { content: contentId } : undefined,
      signal,
    });
  }

  getDecisionLog(runId: string, signal?: AbortSignal) {
    return this.request<DecisionLogRecord[]>(`/decisionlog/${runId}`, { signal });
  }

  getCommands(signal?: AbortSignal) {
    return this.request<CommandInfo[]>('/commands', { signal });
  }

  runCommand(name: string, contentId?: string, signal?: AbortSignal) {
    return this.request<CommandAccepted>('/commands', {
      method: 'POST',
      body: { name, content_id: contentId },
      signal,
    });
  }
}

/** A default same-origin client for app code. Tests construct their own. */
export const api = new CellarrClient();
