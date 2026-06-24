// Types mirroring the daemon's two HTTP surfaces:
//
//   * the native `/api/v1` surface (crates/cellarr-api/src/native.rs) — the
//     compact, snake_case shapes the bespoke screens were first built against;
//   * the Radarr/Sonarr-compatible `/api/v3` shim (crates/cellarr-api/src/shim.rs)
//     — the camelCase resources where the seeded library data actually lives
//     (movies, series, episodes, quality profiles, custom formats, indexers,
//     download clients, root folders, remote path mappings, queue, history,
//     blocklist, commands).
//
// The shapes here are the ones the five screens read — discovered by reading the
// Rust route handlers and by curling the seeded daemon at :9494 — not an
// exhaustive OpenAPI mirror. When the generated OpenAPI spec is wired in, these
// can be replaced by codegen.

export type MediaType = 'movie' | 'tv' | 'music' | 'book' | string;

// ===========================================================================
// Native /api/v1
// ===========================================================================

export interface SystemStatus {
  app_name: string;
  version: string;
  auth_enabled: boolean;
  library_count: number;
  indexer_count: number;
  download_client_count: number;
  /** Filesystem health warnings surfaced by the daemon (may be absent). */
  filesystem_warnings?: string[];
}

export interface Library {
  id: string;
  media_type: MediaType;
  name: string;
  root_folders: string[];
  default_quality_profile: string;
}

export interface ContentRef {
  id: string;
  library_id: string;
  title?: string;
  [key: string]: unknown;
}

export interface ContentNode {
  id: string;
  [key: string]: unknown;
}

export interface MediaFile {
  id: string;
  [key: string]: unknown;
}

export interface HistoryRecord {
  [key: string]: unknown;
}

export interface DecisionLogRecord {
  [key: string]: unknown;
}

export interface CommandInfo {
  name: string;
  description: string;
}

/**
 * A custom format in the native `/api/v1/customformats` shape
 * (snake_case `conditions[]`, `score`). The existing Settings screen reads this;
 * the Radarr-shaped variant is {@link CustomFormatV3}.
 */
export interface CustomFormatCondition {
  kind: string;
  pattern: string;
  required: boolean;
  negate: boolean;
}

export interface NativeCustomFormat {
  id: string;
  name: string;
  score: number;
  conditions: CustomFormatCondition[];
  [key: string]: unknown;
}

/** A native `/api/v1/indexers` config (snake_case, `settings` blob). */
export interface NativeIndexerConfig {
  id: string;
  name: string;
  kind: string;
  protocol: 'usenet' | 'torrent' | string;
  enabled: boolean;
  priority: number;
  settings: Record<string, unknown>;
  [key: string]: unknown;
}

/** A native `/api/v1/downloadclients` config (snake_case, `settings` blob). */
export interface NativeDownloadClientConfig {
  id: string;
  name: string;
  kind: string;
  protocol: 'usenet' | 'torrent' | string;
  enabled: boolean;
  priority: number;
  category?: string;
  settings: Record<string, unknown>;
  [key: string]: unknown;
}

export interface CommandAccepted {
  job_id: string;
  name: string;
  status: string;
}

/** The structured error body the daemon returns: `{ code, message }`. */
export interface ApiErrorBody {
  code: string;
  message: string;
}

// ===========================================================================
// Radarr/Sonarr-compatible /api/v3 shim
// ===========================================================================

/** Detailed system status from the v3 shim (`GET /api/v3/system/status`). */
export interface SystemStatusV3 {
  appName: string;
  instanceName: string;
  version: string;
  buildTime: string;
  startTime: string;
  branch: string;
  authentication: string;
  databaseType: string;
  databaseVersion: string;
  runtimeName: string;
  runtimeVersion: string;
  packageVersion: string;
  osName: string;
  osVersion: string;
  isProduction: boolean;
  isDebug: boolean;
  isDocker: boolean;
  urlBase: string;
  [key: string]: unknown;
}

/** A v3 page envelope (`{ page, pageSize, totalRecords, records }`). */
export interface Page<T> {
  page: number;
  pageSize: number;
  totalRecords: number;
  records: T[];
  sortKey?: string;
  sortDirection?: string;
}

/** A Radarr-style quality reference (`items[].quality`, `formatItems`, …). */
export interface Quality {
  id: number;
  name: string;
  resolution?: number;
  source?: string;
}

/** One row in a profile's quality ladder. */
export interface QualityProfileItem {
  /** A single allowed/denied quality, or a named group (`items` populated). */
  quality?: Quality;
  /** Display name when this row is a group rather than a single quality. */
  name?: string;
  /** Whether this quality/group is allowed by the profile. */
  allowed: boolean;
  /** Nested qualities when this row is a group. */
  items?: QualityProfileItem[];
  /** The numeric id used as the profile cutoff (groups only). */
  id?: number;
}

/** A custom-format scoring entry attached to a profile. */
export interface ProfileFormatItem {
  /** The custom-format id this score applies to. */
  format: number;
  name: string;
  score: number;
}

export interface Language {
  id: number;
  name: string;
}

/**
 * A quality profile exactly as the v3 shim serializes it
 * (`GET /api/v3/qualityprofile`). camelCase, Radarr-compatible. This is the
 * shape that actually carries the seeded HD-1080p / WEB-1080p profiles — the
 * native `/api/v1/qualityprofiles` route returns `[]`.
 */
export interface QualityProfile {
  id: string;
  name: string;
  upgradeAllowed: boolean;
  /** The cutoff quality id (matches an `items[].quality.id` or group id). */
  cutoff: number;
  cutoffFormatScore: number;
  minFormatScore: number;
  minUpgradeFormatScore: number;
  items: QualityProfileItem[];
  formatItems: ProfileFormatItem[];
  language?: Language;
  [key: string]: unknown;
}

/** A v3 custom format with its match specifications. */
export interface CustomFormatField {
  name: string;
  value: unknown;
  order?: number;
}

export interface CustomFormatSpecification {
  name: string;
  implementation: string;
  negate: boolean;
  required: boolean;
  fields: CustomFormatField[];
}

export interface CustomFormatV3 {
  id: number;
  name: string;
  includeCustomFormatWhenRenaming?: boolean;
  specifications: CustomFormatSpecification[];
  [key: string]: unknown;
}

/** A v3 provider field (indexer / download-client settings entry). */
export interface ProviderField {
  name: string;
  value: unknown;
  order?: number;
}

/** A v3 indexer (`GET /api/v3/indexer`). */
export interface IndexerConfigV3 {
  id: number;
  name: string;
  implementation: string;
  implementationName: string;
  configContract: string;
  protocol: 'usenet' | 'torrent' | string;
  priority: number;
  enableRss: boolean;
  enableAutomaticSearch: boolean;
  enableInteractiveSearch: boolean;
  supportsRss: boolean;
  supportsSearch: boolean;
  fields: ProviderField[];
  tags: number[];
  [key: string]: unknown;
}

/** A v3 download client (`GET /api/v3/downloadclient`). */
export interface DownloadClientConfigV3 {
  id: number;
  name: string;
  implementation: string;
  implementationName: string;
  configContract: string;
  protocol: 'usenet' | 'torrent' | string;
  priority: number;
  enable: boolean;
  fields: ProviderField[];
  tags: number[];
  [key: string]: unknown;
}

/** A v3 root folder (`GET /api/v3/rootfolder`). */
export interface RootFolder {
  id: number;
  path: string;
  accessible: boolean;
  freeSpace: number;
  unmappedFolders: Array<{ name?: string; path?: string }>;
  [key: string]: unknown;
}

/** A v3 remote path mapping (`GET /api/v3/remotepathmapping`). */
export interface RemotePathMapping {
  id: number;
  host: string;
  remotePath: string;
  localPath: string;
  [key: string]: unknown;
}

/** A v3 notification provider field (`fields[]` entry / schema field). */
export interface NotificationField {
  name: string;
  value?: unknown;
  label?: string;
  helpText?: string;
  /** SRCL renders password/apiKey privacy fields with a masked input. */
  type?: string;
  privacy?: string;
  advanced?: boolean;
  order?: number;
}

/** A v3 notification (`GET /api/v3/notification`). */
export interface NotificationConfigV3 {
  id: number;
  name: string;
  implementation: string;
  implementationName: string;
  configContract: string;
  onGrab: boolean;
  onDownload: boolean;
  onUpgrade: boolean;
  onRename: boolean;
  onHealthIssue: boolean;
  onHealthRestored: boolean;
  fields: NotificationField[];
  tags: number[];
  [key: string]: unknown;
}

/** A v3 notification connector template (`GET /api/v3/notification/schema`). */
export interface NotificationSchema {
  implementation: string;
  implementationName: string;
  configContract: string;
  fields: NotificationField[];
  [key: string]: unknown;
}

/** A movie file embedded in a v3 movie resource. */
export interface MovieFile {
  path?: string;
  size?: number;
  quality?: { quality?: { name?: string } };
  [key: string]: unknown;
}

/** A v3 movie (`GET /api/v3/movie`). */
export interface Movie {
  id: string;
  title: string;
  titleSlug: string;
  year: number;
  tmdbId: number;
  monitored: boolean;
  hasFile: boolean;
  status: string;
  path: string;
  rootFolderPath: string;
  sizeOnDisk: number;
  qualityProfileId: string | null;
  added: string;
  tags: number[];
  movieFile?: MovieFile;
  overview?: string;
  [key: string]: unknown;
}

/** A v3 series (`GET /api/v3/series`). */
export interface Series {
  id: string;
  title: string;
  titleSlug: string;
  tvdbId: number;
  monitored: boolean;
  hasFile: boolean;
  status: string;
  seriesType: string;
  path: string;
  rootFolderPath: string;
  sizeOnDisk: number;
  qualityProfileId: string | null;
  added: string;
  tags: number[];
  overview?: string;
  [key: string]: unknown;
}

/** A v3 episode (`GET /api/v3/episode?seriesId=…`). */
export interface Episode {
  id: string;
  seriesId?: string;
  seasonNumber?: number;
  episodeNumber?: number;
  title?: string;
  monitored?: boolean;
  hasFile?: boolean;
  airDateUtc?: string;
  [key: string]: unknown;
}

/** A title candidate from `movie/lookup` or `series/lookup`. */
export interface LookupCandidate {
  title: string;
  titleSlug: string;
  year?: number;
  tmdbId?: number;
  tvdbId?: number;
  overview?: string;
  monitored: boolean;
  hasFile: boolean;
  status: string;
  [key: string]: unknown;
}

/** A v3 queue record (`GET /api/v3/queue` → `Page<QueueRecord>`). */
export interface QueueRecord {
  id: string;
  title: string;
  status: string;
  protocol: string;
  trackedDownloadStatus?: string;
  trackedDownloadState?: string;
  size?: number;
  sizeleft?: number;
  timeleft?: string;
  errorMessage?: string;
  [key: string]: unknown;
}

/** A v3 history record (`GET /api/v3/history` → `Page<HistoryRecordV3>`). */
export interface HistoryRecordV3 {
  id?: string;
  eventType?: string;
  date?: string;
  sourceTitle?: string;
  data?: Record<string, unknown>;
  [key: string]: unknown;
}

/** A v3 blocklist record (`GET /api/v3/blocklist` → `Page<BlocklistRecord>`). */
export interface BlocklistRecord {
  id?: string;
  sourceTitle?: string;
  date?: string;
  protocol?: string;
  indexer?: string;
  message?: string;
  [key: string]: unknown;
}

/** A v3 command resource (`GET`/`POST /api/v3/command`). */
export interface CommandResource {
  id: string;
  name: string;
  commandName: string;
  status: string;
  trigger?: string;
  queued?: string;
  started?: string;
  ended?: string;
  [key: string]: unknown;
}

/** A v3 wanted/missing record (`GET /api/v3/wanted/missing`). */
export interface WantedRecord {
  id: string;
  monitored: boolean;
  hasFile: boolean;
  [key: string]: unknown;
}

/** A v3 health check entry (`GET /api/v3/health`). */
export interface HealthCheck {
  type?: string;
  message?: string;
  source?: string;
  [key: string]: unknown;
}

/** A v3 quality definition (`GET /api/v3/qualitydefinition`). */
export interface QualityDefinition {
  id: number;
  title: string;
  weight: number;
  minSize: number | null;
  maxSize: number | null;
  preferredSize: number | null;
  quality: Quality;
  [key: string]: unknown;
}

// ===========================================================================
// Live stream (SSE) — GET /api/v1/stream
// ===========================================================================

/**
 * The tagged domain events pushed over `GET /api/v1/stream`
 * (crates/cellarr-api/src/events.rs). Switch on `type`. The SSE `event:` name
 * matches `type`, so a consumer can either listen for the generic `message`
 * frame or `addEventListener(type, …)`.
 */
export type DomainEvent =
  | { type: 'queue_progress'; grab_id: string; status: string; progress?: number }
  | { type: 'import_completed'; content_id: string; path: string }
  | { type: 'decision_logged'; run_id: string; note: string }
  | { type: 'command_queued'; job_id: string; name: string };

/** The SSE `event:` names a consumer can subscribe to individually. */
export type DomainEventType = DomainEvent['type'];

// ---------------------------------------------------------------------------
// Legacy native-shape aliases (kept for the screens built before the v3 shapes
// were modelled; queue/command screens that read the compact /api/v1 forms).
// ---------------------------------------------------------------------------

/** The compact native queue entry (`GET /api/v1/queue`). */
export interface QueueEntry {
  id: string;
  command: string;
  state: string;
  attempts: number;
}

// The bare names below resolve to the NATIVE /api/v1 shapes, because the
// existing Settings screens were built against them. Screen agents that want the
// Radarr-shaped resources should use the explicit `*V3` types instead.
export type CustomFormat = NativeCustomFormat;
export type IndexerConfig = NativeIndexerConfig;
export type DownloadClientConfig = NativeDownloadClientConfig;
