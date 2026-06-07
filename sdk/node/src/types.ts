export enum HealthStatus {
  Unspecified = 0,
  Healthy = 1,
  Degraded = 2,
  Unhealthy = 3,
}

export enum MetricType {
  Unspecified = 0,
  Counter = 1,
  Gauge = 2,
  Histogram = 3,
  Summary = 4,
}

export interface CapabilityStatus {
  name: string;
  enabled: boolean;
  healthy: boolean;
  last_error: string;
}

export interface CapabilityInfo {
  name: string;
  enabled: boolean;
  version: string;
  dependencies: string[];
  metadata: Record<string, string>;
}

export interface Metric {
  name: string;
  value: number;
  labels: Record<string, string>;
  type: MetricType;
}

export interface StatusResponse {
  health: HealthStatus;
  version: string;
  uptime_seconds: string;
  capabilities: CapabilityStatus[];
}

export interface MetricsResponse {
  metrics: Metric[];
}

export interface ReloadConfigResponse {
  success: boolean;
  message: string;
  warnings: string[];
}

export interface BackupEntry {
  filename: string;
  created_at: string;
  size_bytes: number;
  checksum: string;
}

export interface BackupListResponse {
  backups: BackupEntry[];
}

export interface BackupTriggerResponse {
  success: boolean;
  message: string;
  job_id: string;
}

export interface MigrationRecord {
  version: number;
  name: string;
  applied_at: string;
  checksum: string;
}

export interface MigrationStatusResponse {
  current_version: number;
  applied_migrations: MigrationRecord[];
  pending_count: number;
  is_up_to_date: boolean;
}

export interface MigrationApplyResponse {
  success: boolean;
  message: string;
  migrations_applied: number;
  current_version: number;
}

export interface MigrationRollbackResponse {
  success: boolean;
  message: string;
  rolled_back: MigrationRecord | null;
  current_version: number;
}
