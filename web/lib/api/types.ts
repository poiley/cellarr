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

/** A select option offered by a schema field (`selectOptions[]`). */
export interface CustomFormatSchemaSelectOption {
  value: string;
  name: string;
  order?: number;
}

/** A field in a custom-format schema template (`fields[]`). */
export interface CustomFormatSchemaField {
  name: string;
  label?: string;
  /** `textbox` (free text), `select` (closed choice), or `number`. */
  type?: string;
  order?: number;
  advanced?: boolean;
  unit?: string;
  selectOptions?: CustomFormatSchemaSelectOption[];
}

/**
 * One specification template the CF editor builds rows from
 * (`GET /api/v3/customformat/schema`). `implementation` is the wire id the spec
 * round-trips under (e.g. `SourceSpecification`); `fields[]` describe the inputs
 * the editor renders (a `value` textbox/select, or Size's `min`/`max` numbers).
 */
export interface CustomFormatSchema {
  implementation: string;
  implementationName: string;
  infoLink?: string;
  negate: boolean;
  required: boolean;
  fields: CustomFormatSchemaField[];
  presets?: unknown[];
  [key: string]: unknown;
}

/**
 * One row of the `POST /api/v3/customformat/test` response: a stored format, and
 * whether it matched the supplied release title (with its score).
 */
export interface CustomFormatTestResult {
  id: number;
  name: string;
  matched: boolean;
  score: number;
}

/** Optional pre-parsed fields the CF test preview can override the parse with. */
export interface CustomFormatTestParsed {
  source?: string;
  resolution?: string;
  codec?: string;
  group?: string;
  languages?: string[];
}

/** The body of `POST /api/v3/customformat/test`. */
export interface CustomFormatTestBody {
  title: string;
  parsed?: CustomFormatTestParsed;
  protocol?: 'usenet' | 'torrent';
  indexer_flags?: string[];
  size?: number;
}

/**
 * A delay profile exactly as the v3 shim serializes it
 * (`GET /api/v3/delayprofile`). camelCase, Sonarr/Radarr-compatible: a preferred
 * protocol plus per-protocol grab delays (minutes), a bypass-if-highest flag,
 * tags, and an ordering key.
 */
export interface DelayProfile {
  id: number;
  enableUsenet: boolean;
  enableTorrent: boolean;
  preferredProtocol: 'usenet' | 'torrent' | 'either';
  usenetDelay: number;
  torrentDelay: number;
  bypassIfHighestQuality: boolean;
  tags: string[];
  order: number;
  [key: string]: unknown;
}

/** The v3 delay-profile write body (`POST`/`PUT /api/v3/delayprofile`). */
export interface DelayProfileBody {
  enabled?: boolean;
  preferredProtocol: 'usenet' | 'torrent' | 'either';
  usenetDelay: number;
  torrentDelay: number;
  bypassIfHighestQuality: boolean;
  tags: string[];
  order: number;
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

/**
 * A v3 episode (`GET /api/v3/episode?seriesId=…`). `id`/`seriesId` come back as
 * the numeric projection the v3 list endpoints emit (a JSON number), which the
 * `/episode/monitor` + `/season/monitor` routes accept as their ids. `airDate` is
 * the persisted `content_meta.air_date` (null when unidentified).
 */
export interface Episode {
  id: number | string;
  seriesId?: number | string;
  seasonNumber?: number;
  episodeNumber?: number;
  title?: string;
  monitored?: boolean;
  hasFile?: boolean;
  airDate?: string | null;
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
// Media management / naming config (Radarr/Sonarr-compatible /api/v3/config)
// ===========================================================================

/**
 * The naming config exactly as the v3 shim serializes it
 * (`GET /api/v3/config/naming`). Drives how imported movie / episode files and
 * their containing folders are renamed and laid out on disk.
 */
export interface NamingConfig {
  movieFileFormat: string;
  seriesFolderFormat: string;
  /** Empty string = flat layout (no per-season subfolder). */
  seasonFolderFormat: string;
  episodeFileFormat: string;
  renameEpisodes: boolean;
  renameMovies: boolean;
  seasonFolders: boolean;
  [key: string]: unknown;
}

/**
 * The partial write body for `PUT /api/v3/config/naming`. Every field is
 * optional; the daemon merges what is sent and re-validates each supplied
 * format (a malformed/missing-required-token format → 400, nothing persisted).
 */
export interface NamingConfigBody {
  movieFileFormat?: string;
  seriesFolderFormat?: string;
  seasonFolderFormat?: string;
  episodeFileFormat?: string;
}

/** Which naming target a token belongs to / a preview renders against. */
export type NamingTarget = 'movieFile' | 'seriesFolder' | 'seasonFolder' | 'episodeFile';

/** One renderable token (`{Movie Title}`) advertised by the tokens endpoint. */
export interface NamingToken {
  token: string;
  name: string;
  label: string;
  required: boolean;
  example: string;
}

/** The token vocabulary for one target (`GET /api/v3/config/naming/tokens`). */
export interface NamingTokenTarget {
  target: NamingTarget;
  tokens: NamingToken[];
}

/** The `GET /api/v3/config/naming/tokens` envelope. */
export interface NamingTokens {
  targets: NamingTokenTarget[];
}

/** The body of `POST /api/v3/config/naming/preview`. */
export interface NamingPreviewBody {
  format: string;
  mediaType?: 'movie' | 'tv' | 'series' | 'episode';
  target?: NamingTarget;
  sampleContext?: Record<string, string>;
}

/** The `POST /api/v3/config/naming/preview` response. */
export interface NamingPreview {
  format: string;
  target: NamingTarget;
  rendered: string;
}

/** Unix permissions applied AFTER a media commit (`MediaManagement.permissions`). */
export interface MediaPermissions {
  /** Octal folder mode, e.g. "755". */
  chmodFolder?: string;
  /** Octal file mode, e.g. "644". */
  chmodFile?: string;
  /** Ownership as "user:group". */
  chown?: string;
}

/** Extra non-media sidecar files to import alongside the media. */
export interface ExtraFilesConfig {
  enabled: boolean;
  extensions: string[];
}

/**
 * The persisted media-management settings blob (settings JSON). Naming lives
 * under `naming`; permissions + extra-files apply AFTER the media commit and
 * never roll the imported media back on failure.
 */
export interface MediaManagement {
  recycleBinPath?: string;
  naming?: NamingConfigBody;
  permissions?: MediaPermissions;
  extraFiles?: ExtraFilesConfig;
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
