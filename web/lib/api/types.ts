// Types mirroring the daemon's native `/api/v1` surface (crates/cellarr-api/src/native.rs).
// Kept deliberately light: the shapes the UI reads, not an exhaustive OpenAPI mirror.
// When the generated OpenAPI spec is wired in, these can be replaced by codegen.

export type MediaType = 'movie' | 'tv' | 'music' | 'book' | string;

export interface SystemStatus {
  app_name: string;
  version: string;
  auth_enabled: boolean;
  library_count: number;
  indexer_count: number;
  download_client_count: number;
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

export interface IndexerConfig {
  [key: string]: unknown;
}

export interface DownloadClientConfig {
  [key: string]: unknown;
}

export interface QualityProfile {
  id: string;
  name?: string;
  [key: string]: unknown;
}

export interface CustomFormat {
  [key: string]: unknown;
}

export interface QueueEntry {
  id: string;
  command: string;
  state: string;
  attempts: number;
}

export interface DecisionLogRecord {
  [key: string]: unknown;
}

export interface CommandInfo {
  name: string;
  description: string;
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
