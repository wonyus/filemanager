import { create } from "zustand";
import { commands, formatCommandError } from "../lib/commands";
import type {
  AppInfo,
  BucketSummary,
  ConnectionTestResult,
  ExplorerLocation,
  ListEntriesPage,
  ObjectMetadata,
  ObjectRequest,
  ProfileDetail,
  ProfileDraft,
  ProfileSummary,
  PreviewRequest,
  PreviewResult,
  ShareLink,
  ShareLinkRequest,
  SettingsSnapshot,
  StartTransferRequest,
  TransferHistoryPage,
  TransferJob,
} from "../lib/types";

const DEFAULT_PREVIEW_CACHE_BYTES = 512 * 1024 * 1024;
const MAX_RENDERER_PREVIEW_CACHE_BYTES = 512 * 1024 * 1024;
const DEFAULT_PREVIEW_CACHE_MAX_AGE_HOURS = 24;

interface CachedPreview {
  result: PreviewResult;
  bytes: number;
  lastAccessAt: number;
}

// Preview text is bounded at the backend (2 MiB per request). Keep a small,
// renderer-local LRU for repeat opens, but never cache presigned URLs or binary
// handles because those are bearer credentials with an explicit expiry.
const previewCache = new Map<string, CachedPreview>();
let previewCacheBytes = 0;

function previewCacheKey(
  request: PreviewRequest,
  objectVersion: string | null,
): string {
  return JSON.stringify([
    request.schemaVersion,
    request.profileId,
    request.bucket,
    request.key,
    request.maxBytes ?? null,
    objectVersion,
  ]);
}

function previewObjectVersion(
  request: PreviewRequest,
  metadata: ObjectMetadata | null,
): string | null {
  if (
    !metadata ||
    metadata.profileId !== request.profileId ||
    metadata.bucket !== request.bucket ||
    metadata.key !== request.key
  ) {
    return null;
  }
  return [
    metadata.etag ?? "",
    metadata.versionId ?? "",
    metadata.lastModified ?? "",
    metadata.size ?? "",
  ].join("|");
}

function textByteLength(value: string): number {
  if (typeof TextEncoder !== "undefined") {
    return new TextEncoder().encode(value).byteLength;
  }
  return value.length * 2;
}

function previewCachePolicy(settings: SettingsSnapshot | null) {
  const configuredBytes =
    settings?.previewCacheBytes ??
    settings?.previewCacheQuotaBytes ??
    DEFAULT_PREVIEW_CACHE_BYTES;
  const quotaBytes = Math.min(
    Math.max(0, configuredBytes),
    MAX_RENDERER_PREVIEW_CACHE_BYTES,
  );
  const configuredAgeHours =
    settings?.previewCacheMaxAgeHours ?? DEFAULT_PREVIEW_CACHE_MAX_AGE_HOURS;
  const maxAgeMs = Math.max(1, configuredAgeHours) * 60 * 60 * 1_000;
  return { quotaBytes, maxAgeMs };
}

function removeCachedPreview(key: string) {
  const entry = previewCache.get(key);
  if (!entry) return;
  previewCacheBytes = Math.max(0, previewCacheBytes - entry.bytes);
  previewCache.delete(key);
}

function prunePreviewCache(
  settings: SettingsSnapshot | null,
  now = Date.now(),
) {
  const { quotaBytes, maxAgeMs } = previewCachePolicy(settings);
  for (const [key, entry] of previewCache) {
    if (now - entry.lastAccessAt > maxAgeMs) removeCachedPreview(key);
  }
  if (previewCacheBytes <= quotaBytes) return;
  const oldest = [...previewCache.entries()].sort(
    (left, right) => left[1].lastAccessAt - right[1].lastAccessAt,
  );
  for (const [key] of oldest) {
    if (previewCacheBytes <= quotaBytes) break;
    removeCachedPreview(key);
  }
}

function readCachedPreview(
  request: PreviewRequest,
  settings: SettingsSnapshot | null,
  objectVersion: string | null,
): PreviewResult | null {
  const key = previewCacheKey(request, objectVersion);
  prunePreviewCache(settings);
  const entry = previewCache.get(key);
  if (!entry) return null;
  entry.lastAccessAt = Date.now();
  return entry.result;
}

function writeCachedPreview(
  request: PreviewRequest,
  result: PreviewResult,
  settings: SettingsSnapshot | null,
  objectVersion: string | null,
) {
  if (result.previewKind !== "text" || result.url || !result.text) return;
  const { quotaBytes } = previewCachePolicy(settings);
  const bytes = textByteLength(result.text);
  if (quotaBytes <= 0 || bytes > quotaBytes) return;
  const key = previewCacheKey(request, objectVersion);
  removeCachedPreview(key);
  previewCache.set(key, { result, bytes, lastAccessAt: Date.now() });
  previewCacheBytes += bytes;
  prunePreviewCache(settings);
}

interface AppStore {
  appInfo: AppInfo | null;
  profiles: ProfileSummary[];
  settings: SettingsSnapshot | null;
  activeProfile: ProfileDetail | null;
  selectedProfileId: string | null;
  buckets: BucketSummary[];
  listing: ListEntriesPage | null;
  location: ExplorerLocation | null;
  metadata: ObjectMetadata | null;
  preview: PreviewResult | null;
  shareLink: ShareLink | null;
  transfers: TransferHistoryPage | null;
  transferLoading: boolean;
  loading: boolean;
  saving: boolean;
  testing: boolean;
  error: string | null;
  /**
   * Explorer-specific errors are kept separate from the global command error
   * so a failed transfer/profile action cannot make a healthy listing look
   * empty.  The UI uses these fields to offer a targeted retry and to label
   * stale/partial pages accurately.
   */
  listingError: string | null;
  bucketError: string | null;
  testResult: ConnectionTestResult | null;
  listingGeneration: number;
  profileSelectionGeneration: number;
  inspectorGeneration: number;
  bootstrap: () => Promise<void>;
  selectProfile: (id: string) => Promise<void>;
  saveProfile: (
    draft: ProfileDraft,
    profileId?: string,
    expectedRevision?: number,
  ) => Promise<ProfileDetail | null>;
  duplicateProfile: (id: string) => Promise<ProfileDetail | null>;
  deleteProfile: (id: string) => Promise<boolean>;
  testProfile: (draft: ProfileDraft) => Promise<ConnectionTestResult | null>;
  openExplorer: (location: ExplorerLocation) => Promise<void>;
  listBuckets: (profileId: string) => Promise<void>;
  listEntries: (
    location: ExplorerLocation,
    continuationToken?: string,
  ) => Promise<void>;
  loadMetadata: (request: ObjectRequest) => Promise<ObjectMetadata | null>;
  loadPreview: (request: PreviewRequest) => Promise<PreviewResult | null>;
  createShare: (request: ShareLinkRequest) => Promise<ShareLink | null>;
  clearPreview: () => void;
  clearShare: () => void;
  clearInspector: () => void;
  clearListingError: () => void;
  clearBucketError: () => void;
  refreshTransfers: () => Promise<TransferHistoryPage | null>;
  startTransfer: (request: StartTransferRequest) => Promise<TransferJob | null>;
  pauseTransfer: (id: string) => Promise<void>;
  resumeTransfer: (id: string) => Promise<void>;
  cancelTransfer: (id: string) => Promise<void>;
  retryTransfer: (id: string) => Promise<void>;
  clearTransferHistory: () => Promise<void>;
  clearError: () => void;
}

const refreshProfiles = async (set: (state: Partial<AppStore>) => void) => {
  const profiles = await commands.listProfiles();
  set({ profiles });
  return profiles;
};

export const useAppStore = create<AppStore>((set, get) => ({
  appInfo: null,
  profiles: [],
  settings: null,
  activeProfile: null,
  selectedProfileId: null,
  buckets: [],
  listing: null,
  location: null,
  metadata: null,
  preview: null,
  shareLink: null,
  transfers: null,
  transferLoading: false,
  loading: false,
  saving: false,
  testing: false,
  error: null,
  listingError: null,
  bucketError: null,
  testResult: null,
  listingGeneration: 0,
  profileSelectionGeneration: 0,
  inspectorGeneration: 0,

  bootstrap: async () => {
    set({ loading: true, error: null });
    try {
      const [appInfo, profiles, settings] = await Promise.all([
        commands.getAppInfo(),
        commands.listProfiles(),
        commands.getSettings(),
      ]);
      set({ appInfo, profiles, settings, loading: false });
    } catch (error) {
      set({ loading: false, error: formatCommandError(error) });
    }
  },

  selectProfile: async (id) => {
    const requestGeneration = get().profileSelectionGeneration + 1;
    const inspectorGeneration = get().inspectorGeneration + 1;
    set({
      loading: true,
      error: null,
      selectedProfileId: id,
      activeProfile: null,
      buckets: [],
      listing: null,
      location: null,
      listingError: null,
      bucketError: null,
      metadata: null,
      preview: null,
      shareLink: null,
      profileSelectionGeneration: requestGeneration,
      inspectorGeneration,
    });
    try {
      const activeProfile = await commands.getProfile(id);
      if (
        get().profileSelectionGeneration === requestGeneration &&
        get().selectedProfileId === id
      ) {
        set({ activeProfile, loading: false });
      }
    } catch (error) {
      if (
        get().profileSelectionGeneration === requestGeneration &&
        get().selectedProfileId === id
      ) {
        set({ loading: false, error: formatCommandError(error) });
      }
    }
  },

  saveProfile: async (draft, profileId, expectedRevision) => {
    set({ saving: true, error: null });
    try {
      const detail = profileId
        ? await commands.updateProfile(profileId, expectedRevision ?? 1, draft)
        : await commands.createProfile(draft);
      const profiles = await refreshProfiles(set);
      set({
        saving: false,
        activeProfile: detail,
        selectedProfileId: detail.id,
        profiles,
        testResult: null,
      });
      return detail;
    } catch (error) {
      set({ saving: false, error: formatCommandError(error) });
      return null;
    }
  },

  duplicateProfile: async (id) => {
    set({ saving: true, error: null });
    try {
      const detail = await commands.duplicateProfile(id);
      const profiles = await refreshProfiles(set);
      set({
        saving: false,
        activeProfile: detail,
        selectedProfileId: detail.id,
        profiles,
      });
      return detail;
    } catch (error) {
      set({ saving: false, error: formatCommandError(error) });
      return null;
    }
  },

  deleteProfile: async (id) => {
    set({ saving: true, error: null });
    try {
      await commands.deleteProfile(id);
      const profiles = await refreshProfiles(set);
      const selectedProfileId =
        get().selectedProfileId === id ? null : get().selectedProfileId;
      const inspectorGeneration = get().inspectorGeneration + 1;
      set({
        saving: false,
        profiles,
        selectedProfileId,
        activeProfile: selectedProfileId ? get().activeProfile : null,
        buckets: selectedProfileId ? get().buckets : [],
        listing: selectedProfileId ? get().listing : null,
        location: selectedProfileId ? get().location : null,
        metadata: selectedProfileId ? get().metadata : null,
        preview: selectedProfileId ? get().preview : null,
        shareLink: selectedProfileId ? get().shareLink : null,
        inspectorGeneration,
      });
      return true;
    } catch (error) {
      set({ saving: false, error: formatCommandError(error) });
      return false;
    }
  },

  testProfile: async (draft) => {
    set({ testing: true, error: null, testResult: null });
    try {
      const testResult = await commands.testProfile(draft);
      set({ testing: false, testResult });
      return testResult;
    } catch (error) {
      set({ testing: false, error: formatCommandError(error) });
      return null;
    }
  },

  openExplorer: async (location) => {
    const inspectorGeneration = get().inspectorGeneration + 1;
    set({
      location,
      listing: null,
      metadata: null,
      preview: null,
      shareLink: null,
      listingError: null,
      inspectorGeneration,
      error: null,
    });
    await get().listEntries(location);
  },

  listBuckets: async (profileId) => {
    const requestGeneration = get().profileSelectionGeneration;
    set({ loading: true, error: null, bucketError: null });
    try {
      const buckets = await commands.listBuckets(profileId);
      if (
        get().profileSelectionGeneration === requestGeneration &&
        get().selectedProfileId === profileId
      ) {
        set({ buckets, loading: false, bucketError: null });
      }
    } catch (error) {
      if (
        get().profileSelectionGeneration === requestGeneration &&
        get().selectedProfileId === profileId
      ) {
        const message = formatCommandError(error);
        set({ loading: false, error: message, bucketError: message });
      }
    }
  },

  listEntries: async (location, continuationToken) => {
    const requestGeneration = get().listingGeneration + 1;
    const inspectorGeneration = get().inspectorGeneration + 1;
    set({
      loading: true,
      error: null,
      listingError: null,
      location,
      metadata: null,
      preview: null,
      shareLink: null,
      listingGeneration: requestGeneration,
      inspectorGeneration,
    });
    try {
      const listing = await commands.listEntries({
        schemaVersion: 1,
        location,
        continuationToken,
        pageSize: 200,
        requestGeneration,
      });
      // A slow response from a previous location must never replace the current view.
      if (
        get().listingGeneration === requestGeneration &&
        get().location?.profileId === location.profileId &&
        get().location?.bucket === location.bucket &&
        get().location?.prefix === location.prefix
      ) {
        set({ listing, loading: false, listingError: null });
      }
    } catch (error) {
      if (get().listingGeneration === requestGeneration) {
        const message = formatCommandError(error);
        set({ loading: false, error: message, listingError: message });
      }
    }
  },

  loadMetadata: async (request) => {
    const inspectorGeneration = get().inspectorGeneration + 1;
    set({
      loading: true,
      error: null,
      metadata: null,
      preview: null,
      shareLink: null,
      inspectorGeneration,
    });
    try {
      const metadata = await commands.headObject(request);
      if (get().inspectorGeneration !== inspectorGeneration) return null;
      set({ loading: false, metadata });
      return metadata;
    } catch (error) {
      if (get().inspectorGeneration === inspectorGeneration) {
        set({ loading: false, error: formatCommandError(error) });
      }
      return null;
    }
  },

  loadPreview: async (request) => {
    const inspectorGeneration = get().inspectorGeneration + 1;
    set({
      loading: true,
      error: null,
      preview: null,
      inspectorGeneration,
    });
    const objectVersion = previewObjectVersion(request, get().metadata);
    const cached = readCachedPreview(request, get().settings, objectVersion);
    if (cached) {
      if (get().inspectorGeneration !== inspectorGeneration) return null;
      set({ loading: false, preview: cached });
      return cached;
    }
    try {
      const preview = await commands.previewObject(request);
      if (get().inspectorGeneration !== inspectorGeneration) return null;
      writeCachedPreview(
        request,
        preview,
        get().settings,
        previewObjectVersion(request, get().metadata),
      );
      set({ loading: false, preview });
      return preview;
    } catch (error) {
      if (get().inspectorGeneration === inspectorGeneration) {
        set({ loading: false, error: formatCommandError(error) });
      }
      return null;
    }
  },

  createShare: async (request) => {
    const inspectorGeneration = get().inspectorGeneration + 1;
    set({
      loading: true,
      error: null,
      shareLink: null,
      inspectorGeneration,
    });
    try {
      const shareLink = await commands.createShareLink(request);
      if (get().inspectorGeneration !== inspectorGeneration) return null;
      set({ loading: false, shareLink });
      return shareLink;
    } catch (error) {
      if (get().inspectorGeneration === inspectorGeneration) {
        set({ loading: false, error: formatCommandError(error) });
      }
      return null;
    }
  },

  clearInspector: () =>
    set((state) => ({
      metadata: null,
      preview: null,
      shareLink: null,
      inspectorGeneration: state.inspectorGeneration + 1,
    })),

  clearPreview: () =>
    set((state) => ({
      preview: null,
      inspectorGeneration: state.inspectorGeneration + 1,
    })),

  clearShare: () =>
    set((state) => ({
      shareLink: null,
      inspectorGeneration: state.inspectorGeneration + 1,
    })),

  clearListingError: () => set({ listingError: null }),

  clearBucketError: () => set({ bucketError: null }),

  refreshTransfers: async () => {
    set({ transferLoading: true, error: null });
    try {
      const transfers = await commands.listTransfers(true);
      set({ transfers, transferLoading: false });
      return transfers;
    } catch (error) {
      set({ transferLoading: false, error: formatCommandError(error) });
      return null;
    }
  },

  startTransfer: async (request) => {
    set({ transferLoading: true, error: null });
    try {
      const job = await commands.startTransfer(request);
      const transfers = await commands.listTransfers(true);
      set({ transfers, transferLoading: false });
      return job;
    } catch (error) {
      set({ transferLoading: false, error: formatCommandError(error) });
      return null;
    }
  },

  pauseTransfer: async (id) => transferAction(commands.pauseTransfer, id, set),
  resumeTransfer: async (id) =>
    transferAction(commands.resumeTransfer, id, set),
  cancelTransfer: async (id) =>
    transferAction(commands.cancelTransfer, id, set),
  retryTransfer: async (id) => transferAction(commands.retryTransfer, id, set),

  clearTransferHistory: async () => {
    set({ transferLoading: true, error: null });
    try {
      await commands.clearTransferHistory();
      const transfers = await commands.listTransfers(true);
      set({ transfers, transferLoading: false });
    } catch (error) {
      set({ transferLoading: false, error: formatCommandError(error) });
    }
  },

  clearError: () => set({ error: null }),
}));

async function transferAction(
  action: (id: string) => Promise<TransferJob>,
  id: string,
  set: (state: Partial<AppStore>) => void,
) {
  set({ transferLoading: true, error: null });
  try {
    await action(id);
    const transfers = await commands.listTransfers(true);
    set({ transfers, transferLoading: false });
  } catch (error) {
    set({ transferLoading: false, error: formatCommandError(error) });
  }
}
