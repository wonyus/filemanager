import { invoke } from "@tauri-apps/api/core";
import type {
  AppInfo,
  BucketSummary,
  Bookmark,
  ConnectionTestResult,
  DiagnosticsExportRequest,
  DiagnosticsExportResult,
  ObjectMetadata,
  MetadataEditRequest,
  MetadataEditResult,
  ObjectRequest,
  ListEntriesPage,
  ListEntriesRequest,
  LogDirectoryResult,
  PreviewRequest,
  PreviewResult,
  RecentLocation,
  ProfileSummary,
  ProfileDetail,
  ProfileDraft,
  ProfileExportResult,
  ProfileImportResult,
  PublicError,
  ShareLink,
  ShareLinkRequest,
  SettingsSnapshot,
  StartTransferRequest,
  TransferHistoryPage,
  TransferDetails,
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
  exportProfiles: (profileIds: string[], destinationPath: string) =>
    invoke<ProfileExportResult>("export_profiles", {
      request: { schemaVersion: 1, profileIds, destinationPath },
    }),
  importProfiles: (sourcePath: string) =>
    invoke<ProfileImportResult>("import_profiles", {
      request: { schemaVersion: 1, sourcePath },
    }),
  testProfile: (draft: ProfileDraft) =>
    invoke<ConnectionTestResult>("test_profile", { draft }),
  listBuckets: (profileId: string) =>
    invoke<BucketSummary[]>("list_buckets", { profileId }),
  listEntries: (request: ListEntriesRequest) =>
    invoke<ListEntriesPage>("list_entries", { request }),
  addBookmark: (request: {
    schemaVersion: number;
    profileId: string;
    bucket: string;
    prefix: string;
    name: string;
    sortOrder?: number;
  }) => invoke<Bookmark>("add_bookmark", { request }),
  listBookmarks: (profileId: string) =>
    invoke<Bookmark[]>("list_bookmarks", {
      request: { schemaVersion: 1, profileId },
    }),
  removeBookmark: (id: number) =>
    invoke<void>("remove_bookmark", { request: { id } }),
  recordRecentLocation: (location: {
    profileId: string;
    bucket: string;
    prefix: string;
  }) =>
    invoke<RecentLocation>("record_recent_location", {
      request: { schemaVersion: 1, location },
    }),
  listRecentLocations: (profileId: string) =>
    invoke<RecentLocation[]>("list_recent_locations", {
      request: { schemaVersion: 1, profileId, limit: 30 },
    }),
  headObject: (request: ObjectRequest) =>
    invoke<ObjectMetadata>("head_object", { request }),
  editMetadata: (request: MetadataEditRequest) =>
    invoke<MetadataEditResult>("edit_metadata", { request }),
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
  getTransferDetails: (transferId: string) =>
    invoke<TransferDetails>("get_transfer_details", { transferId }),
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
  interruptActiveTransfers: () => invoke<number>("interrupt_active_transfers"),
  updateSettings: (patch: Record<string, unknown>) =>
    invoke<SettingsSnapshot>("update_settings", { patch }),
  resetSettings: () => invoke<SettingsSnapshot>("reset_settings"),
  openLogDirectory: () => invoke<LogDirectoryResult>("open_log_directory"),
  exportDiagnostics: (request: DiagnosticsExportRequest) =>
    invoke<DiagnosticsExportResult>("export_diagnostics", { request }),
  clearLogs: () => invoke<number>("clear_logs"),
  checkForUpdates: () => invoke<UpdateCheckResult>("check_for_updates"),
  pickFile: () => invoke<string | null>("pick_file"),
  pickDirectory: () => invoke<string | null>("pick_directory"),
  pickSaveFile: (defaultName?: string) =>
    invoke<string | null>("pick_save_file", { defaultName }),
  openDestinationFolder: (path: string) =>
    invoke<void>("open_destination_folder", { path }),
};
