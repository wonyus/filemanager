import { invoke } from "@tauri-apps/api/core";
import type {
  AppInfo,
  BucketSummary,
  ConnectionTestResult,
  DiagnosticsExportRequest,
  DiagnosticsExportResult,
  ObjectMetadata,
  ObjectRequest,
  ListEntriesPage,
  ListEntriesRequest,
  LogDirectoryResult,
  PreviewRequest,
  PreviewResult,
  ProfileSummary,
  ProfileDetail,
  ProfileDraft,
  PublicError,
  ShareLink,
  ShareLinkRequest,
  SettingsSnapshot,
  StartTransferRequest,
  TransferHistoryPage,
  TransferJob,
  UpdateCheckResult,
} from "./types";

export function isPublicError(value: unknown): value is PublicError {
  return (
    typeof value === "object" &&
    value !== null &&
    "code" in value &&
    "message" in value &&
    typeof value.message === "string"
  );
}

export function formatCommandError(value: unknown): string {
  if (isPublicError(value)) return value.message;
  if (value instanceof Error) return value.message;
  return "The operation could not be completed.";
}

export const commands = {
  getAppInfo: () => invoke<AppInfo>("get_app_info"),
  listProfiles: () => invoke<ProfileSummary[]>("list_profiles"),
  getSettings: () => invoke<SettingsSnapshot>("get_settings"),
  getProfile: (id: string) => invoke<ProfileDetail>("get_profile", { id }),
  createProfile: (draft: ProfileDraft) =>
    invoke<ProfileDetail>("create_profile", { draft }),
  updateProfile: (id: string, expectedRevision: number, draft: ProfileDraft) =>
    invoke<ProfileDetail>("update_profile", {
      id,
      expectedRevision,
      draft,
    }),
  duplicateProfile: (id: string) =>
    invoke<ProfileDetail>("duplicate_profile", { id }),
  deleteProfile: (id: string) =>
    invoke<void>("delete_profile", { id, confirmation: "DELETE" }),
  testProfile: (draft: ProfileDraft) =>
    invoke<ConnectionTestResult>("test_profile", { draft }),
  listBuckets: (profileId: string) =>
    invoke<BucketSummary[]>("list_buckets", { profileId }),
  listEntries: (request: ListEntriesRequest) =>
    invoke<ListEntriesPage>("list_entries", { request }),
  headObject: (request: ObjectRequest) =>
    invoke<ObjectMetadata>("head_object", { request }),
  previewObject: (request: PreviewRequest) =>
    invoke<PreviewResult>("preview_object", { request }),
  createShareLink: (request: ShareLinkRequest) =>
    invoke<ShareLink>("create_share_link", { request }),
  startTransfer: (request: StartTransferRequest) =>
    invoke<TransferJob>("start_transfer", { request }),
  listTransfers: (includeActive = true) =>
    invoke<TransferHistoryPage>("list_transfers", {
      request: {
        schemaVersion: 1,
        includeActive,
        limit: 100,
        offset: 0,
      },
    }),
  pauseTransfer: (transferId: string) =>
    invoke<TransferJob>("pause_transfer", { transferId }),
  resumeTransfer: (transferId: string) =>
    invoke<TransferJob>("resume_transfer", { transferId }),
  cancelTransfer: (transferId: string) =>
    invoke<TransferJob>("cancel_transfer", { transferId }),
  retryTransfer: (transferId: string) =>
    invoke<TransferJob>("retry_transfer", { transferId }),
  clearTransferHistory: () =>
    invoke<number>("clear_transfer_history", {
      request: { schemaVersion: 1, before: null, includeFailed: true },
    }),
  updateSettings: (patch: Record<string, unknown>) =>
    invoke<SettingsSnapshot>("update_settings", { patch }),
  resetSettings: () => invoke<SettingsSnapshot>("reset_settings"),
  openLogDirectory: () => invoke<LogDirectoryResult>("open_log_directory"),
  exportDiagnostics: (request: DiagnosticsExportRequest) =>
    invoke<DiagnosticsExportResult>("export_diagnostics", { request }),
  clearLogs: () => invoke<number>("clear_logs"),
  checkForUpdates: () => invoke<UpdateCheckResult>("check_for_updates"),
};
