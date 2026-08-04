export type ProviderType =
  "awsS3" | "cloudflareR2" | "minio" | "wasabi" | "customS3";

export type CredentialMode = "static" | "temporarySession";
export type AddressingStyle = "virtualHosted" | "path";

export type TransferOperation =
  | "uploadFile"
  | "uploadDirectory"
  | "downloadFile"
  | "downloadPrefix"
  | "copyObject"
  | "copyPrefix"
  | "moveObject"
  | "movePrefix"
  | "deleteObjects";
export type TransferStatus =
  | "queued"
  | "planning"
  | "waitingForUser"
  | "running"
  | "pausing"
  | "paused"
  | "retrying"
  | "cancelling"
  | "completed"
  | "completedWithWarnings"
  | "failed"
  | "cancelled"
  | "interrupted";
export type CollisionPolicy = "ask" | "replace" | "skip" | "fail";

export type TransferEndpoint =
  | { kind: "remote"; profileId: string; bucket: string; key: string }
  | { kind: "local"; path: string };

export interface StartTransferRequest {
  schemaVersion?: number;
  operation: TransferOperation;
  profileId?: string;
  source: TransferEndpoint;
  destination?: TransferEndpoint;
  collisionPolicy?: CollisionPolicy;
  totalBytes?: number;
  totalItems?: number;
  confirmation?: string;
  recursive?: boolean;
}

export interface TransferJob {
  schemaVersion: number;
  id: string;
  operation: TransferOperation;
  profileId?: string;
  source: TransferEndpoint;
  destination?: TransferEndpoint;
  status: TransferStatus;
  collisionPolicy: CollisionPolicy;
  totalBytes?: number;
  transferredBytes: number;
  totalItems?: number;
  completedItems: number;
  failedItems: number;
  speedBps?: number;
  etaSeconds?: number;
  retryCount: number;
  createdAt: string;
  startedAt?: string;
  finishedAt?: string;
  error?: PublicError;
}

export interface TransferSummary {
  schemaVersion: number;
  id: string;
  operation: TransferOperation;
  status: TransferStatus;
  transferredBytes: number;
  totalBytes?: number;
  completedItems: number;
  totalItems?: number;
  createdAt: string;
  finishedAt?: string;
}

export interface TransferHistoryPage {
  schemaVersion: number;
  items: TransferSummary[];
  total: number;
  limit: number;
  offset: number;
}

export type AppErrorCode =
  | "VALIDATION_FAILED"
  | "PROFILE_NOT_FOUND"
  | "PROFILE_REVISION_CONFLICT"
  | "INVALID_ENDPOINT"
  | "INSECURE_ENDPOINT_BLOCKED"
  | "CREDENTIAL_MISSING"
  | "CREDENTIAL_EXPIRED"
  | "CREDENTIAL_REJECTED"
  | "BUCKET_NOT_FOUND"
  | "BUCKET_ACCESS_DENIED"
  | "OBJECT_NOT_FOUND"
  | "DESTINATION_EXISTS"
  | "ROOT_PREFIX_VIOLATION"
  | "UNSUPPORTED_PROVIDER_FEATURE"
  | "NETWORK_UNAVAILABLE"
  | "REQUEST_TIMED_OUT"
  | "TLS_ERROR"
  | "RATE_LIMITED"
  | "PROVIDER_UNAVAILABLE"
  | "LOCAL_PATH_INVALID"
  | "LOCAL_PERMISSION_DENIED"
  | "LOCAL_DISK_FULL"
  | "TRANSFER_CANCELLED"
  | "TRANSFER_STATE_CONFLICT"
  | "DATABASE_ERROR"
  | "CREDENTIAL_STORE_UNAVAILABLE"
  | "UPDATE_VERIFICATION_FAILED"
  | "UNKNOWN";

export interface PublicError {
  schemaVersion?: number;
  code: AppErrorCode;
  message: string;
  retryable: boolean;
  requestId?: string;
  providerRequestId?: string;
  correlationId?: string;
  fieldErrors?: Record<string, string>;
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
  schemaVersion: number;
  id: string;
  name: string;
  provider: ProviderType;
  endpointDisplay?: string;
  region: string;
  defaultBucket?: string;
  rootPrefix: string;
  favorite: boolean;
  lastConnectedAt?: string;
  credentialState: "configured" | "missing" | "unavailable";
  connectionState: "unknown" | "connected" | "failed";
}

export interface ProfileDraft {
  schemaVersion: number;
  id?: string;
  name: string;
  provider: ProviderType;
  accountId?: string;
  endpoint?: string;
  region: string;
  credentialMode: CredentialMode;
  accessKeyId?: string;
  secretAccessKey?: string;
  sessionToken?: string;
  defaultBucket?: string;
  rootPrefix?: string;
  addressingStyle?: AddressingStyle;
  allowInsecureHttp: boolean;
  favorite: boolean;
}

export interface ProfileDetail {
  schemaVersion: number;
  id: string;
  name: string;
  provider: ProviderType;
  endpoint?: string;
  region: string;
  credentialMode: CredentialMode;
  accessKeyPreview?: string;
  hasSecretAccessKey: boolean;
  hasSessionToken: boolean;
  defaultBucket?: string;
  rootPrefix?: string;
  addressingStyle: AddressingStyle;
  allowInsecureHttp: boolean;
  favorite: boolean;
  favoriteOrder: number;
  revision: number;
}

export interface ConnectionTestResult {
  schemaVersion: number;
  success: boolean;
  latencyMs: number;
  bucketAccess: boolean;
  message: string;
  providerRequestId?: string;
  canListBuckets?: boolean;
  canHeadBucket?: boolean;
  supportsMultipartUpload?: boolean;
  supportsPresignedGet?: boolean;
}

export interface BucketSummary {
  schemaVersion: number;
  name: string;
  creationDate?: string;
}

export interface ExplorerLocation {
  profileId: string;
  bucket: string;
  prefix: string;
}

export type EntryKind = "file" | "prefix" | "folderMarker";

export interface EntrySummary {
  schemaVersion: number;
  id: string;
  kind: EntryKind;
  displayName: string;
  key: string;
  size?: number;
  lastModified?: string;
  storageClass?: string;
  contentTypeHint?: string;
  isFolderMarker: boolean;
}

export interface ListEntriesRequest {
  schemaVersion: number;
  location: ExplorerLocation;
  continuationToken?: string;
  pageSize: number;
  requestGeneration: number;
}

export interface ListEntriesPage {
  schemaVersion: number;
  requestGeneration: number;
  location: ExplorerLocation;
  entries: EntrySummary[];
  nextToken?: string;
  isComplete: boolean;
  providerRequestId?: string;
}

export interface ObjectRequest {
  schemaVersion: number;
  profileId: string;
  bucket: string;
  key: string;
}

export interface PreviewRequest extends ObjectRequest {
  maxBytes?: number;
}

export interface ShareLinkRequest extends ObjectRequest {
  expiresInSeconds?: number;
}

export interface ObjectMetadata {
  schemaVersion: number;
  profileId: string;
  bucket: string;
  key: string;
  size?: number;
  etag?: string;
  versionId?: string;
  lastModified?: string;
  storageClass?: string;
  contentType?: string;
  contentDisposition?: string;
  cacheControl?: string;
  contentEncoding?: string;
  contentLanguage?: string;
  expires?: string;
  checksumSha256?: string;
  checksumSha1?: string;
  checksumCrc32?: string;
  checksumCrc32c?: string;
  encryption?: string;
  userMetadata: Record<string, string>;
  previewSupported: boolean;
  previewKind?: "text" | "image" | "audio" | "video" | "pdf";
  previewReason?: string;
}

export interface PreviewResult {
  schemaVersion: number;
  profileId: string;
  bucket: string;
  key: string;
  previewKind: "text" | "image" | "audio" | "video" | "pdf";
  contentType: string;
  text: string;
  url?: string;
  expiresAt?: string;
  bytesRead: number;
  totalSize?: number;
  truncated: boolean;
}

export interface ShareLink {
  schemaVersion: number;
  profileId: string;
  bucket: string;
  key: string;
  url: string;
  expiresAt: string;
  expiresInSeconds: number;
}

export interface SettingsSnapshot {
  schemaVersion: number;
  concurrentJobs?: number;
  perJobPartConcurrency?: number;
  perProfileRequestLimit?: number;
  multipartThresholdBytes?: number;
  initialPartSizeBytes?: number;
  retryBaseDelayMs?: number;
  retryMaxDelayMs?: number;
  progressHz?: number;
  defaultCollisionPolicy?: CollisionPolicy;
  preserveEmptyFolders?: boolean;
  keepPartialDownloads?: boolean;
  previewCacheBytes?: number;
  previewCacheMaxAgeHours?: number;
  transferHistoryDays?: number;
  transferHistoryMaxJobs?: number;
  logRetentionDays?: number;
  logMaxBytes?: number;
  typedConfirmObjectThreshold?: number;
  typedConfirmBytesThreshold?: number;
  updateChannel?: "stable" | "beta";
  automaticUpdateCheck?: boolean;
  transferConcurrency: number;
  partConcurrency: number;
  retryLimit: number;
  previewCacheQuotaBytes: number;
}

export interface DiagnosticsExportRequest {
  schemaVersion: number;
  destinationPath: string;
}

export interface DiagnosticsExportResult {
  schemaVersion: number;
  path: string;
  bytesWritten: number;
  redacted: boolean;
}

export interface LogDirectoryResult {
  schemaVersion: number;
  path: string;
}

export interface UpdateCheckResult {
  schemaVersion: number;
  channel: "stable" | "beta";
  available: boolean;
  message: string;
}
