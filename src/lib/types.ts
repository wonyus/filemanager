export type ProviderType =
  "awsS3" | "cloudflareR2" | "minio" | "wasabi" | "customS3";

export type AppErrorCode =
  | "VALIDATION_FAILED"
  | "PROFILE_NOT_FOUND"
  | "DATABASE_ERROR"
  | "CREDENTIAL_STORE_UNAVAILABLE"
  | "UNKNOWN";

export interface PublicError {
  code: AppErrorCode;
  message: string;
  retryable: boolean;
  requestId?: string;
  details: Record<string, unknown>;
}

export interface AppInfo {
  productName: string;
  version: string;
  schemaVersion: number;
  phase:
    "foundation" | "profiles" | "transfers" | "largeOperations" | "packaging";
}

export interface ProfileSummary {
  id: string;
  name: string;
  provider: ProviderType;
  region: string;
  defaultBucket?: string;
  favorite: boolean;
  lastConnectedAt?: string;
}

export interface SettingsSnapshot {
  schemaVersion: number;
  transferConcurrency: number;
  partConcurrency: number;
  retryLimit: number;
  previewCacheQuotaBytes: number;
}
