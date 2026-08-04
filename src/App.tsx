import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from "react";
import { commands, formatCommandError } from "./lib/commands";
import { useAppStore } from "./stores/appStore";
import type {
  AddressingStyle,
  CollisionPolicy,
  CredentialMode,
  EntrySummary,
  ExplorerLocation,
  ObjectMetadata,
  ProfileDetail,
  ProfileDraft,
  ProfileSummary,
  PreviewResult,
  ProviderType,
  ShareLink,
  SettingsSnapshot,
  StartTransferRequest,
  TransferEndpoint,
  TransferHistoryPage,
  TransferJob,
  TransferOperation,
} from "./lib/types";

const providerLabels: Record<ProviderType, string> = {
  awsS3: "Amazon S3",
  cloudflareR2: "Cloudflare R2",
  minio: "MinIO",
  wasabi: "Wasabi",
  customS3: "Custom S3",
};

const navigation = [
  { label: "Profiles", icon: "◉" },
  { label: "Explorer", icon: "▦" },
  { label: "Transfers", icon: "⇄" },
  { label: "Settings", icon: "⚙" },
];

function emptyDraft(): ProfileDraft {
  return {
    schemaVersion: 1,
    name: "",
    provider: "awsS3",
    accountId: "",
    endpoint: "",
    region: "",
    credentialMode: "static",
    accessKeyId: "",
    secretAccessKey: "",
    sessionToken: "",
    defaultBucket: "",
    rootPrefix: "",
    addressingStyle: "virtualHosted",
    allowInsecureHttp: false,
    favorite: false,
  };
}

function draftFromProfile(profile: ProfileDetail): ProfileDraft {
  return {
    ...emptyDraft(),
    id: profile.id,
    name: profile.name,
    provider: profile.provider,
    endpoint: profile.endpoint ?? "",
    region: profile.region,
    credentialMode: profile.credentialMode,
    defaultBucket: profile.defaultBucket ?? "",
    rootPrefix: profile.rootPrefix ?? "",
    addressingStyle: profile.addressingStyle,
    allowInsecureHttp: profile.allowInsecureHttp,
    favorite: profile.favorite,
  };
}

function App() {
  const {
    appInfo,
    profiles,
    settings,
    activeProfile,
    selectedProfileId,
    buckets,
    listing,
    location,
    metadata,
    preview,
    shareLink,
    transfers,
    transferLoading,
    loading,
    saving,
    testing,
    error,
    testResult,
    bootstrap,
    selectProfile,
    saveProfile,
    duplicateProfile,
    deleteProfile,
    testProfile,
    listBuckets,
    openExplorer,
    listEntries,
    loadMetadata,
    loadPreview,
    createShare,
    clearPreview,
    clearShare,
    clearError,
    refreshTransfers,
    startTransfer,
    pauseTransfer,
    resumeTransfer,
    cancelTransfer,
    retryTransfer,
    clearTransferHistory,
  } = useAppStore();
  const [editorOpen, setEditorOpen] = useState(false);
  const [editingProfile, setEditingProfile] = useState<ProfileDetail | null>(
    null,
  );
  const [activeSection, setActiveSection] = useState("profiles");

  useEffect(() => {
    void bootstrap();
    void refreshTransfers();
  }, [bootstrap, refreshTransfers]);

  useEffect(() => {
    const expiryTimes = [preview?.expiresAt, shareLink?.expiresAt]
      .filter((value): value is string => Boolean(value))
      .map((value) => Date.parse(value))
      .filter((value) => Number.isFinite(value));
    if (expiryTimes.length === 0) return;
    const delay = Math.max(0, Math.min(...expiryTimes) - Date.now());
    const timer = window.setTimeout(() => {
      if (preview?.expiresAt && Date.parse(preview.expiresAt) <= Date.now()) {
        clearPreview();
      }
      if (
        shareLink?.expiresAt &&
        Date.parse(shareLink.expiresAt) <= Date.now()
      ) {
        clearShare();
      }
    }, delay);
    return () => window.clearTimeout(timer);
  }, [preview, shareLink, clearPreview, clearShare]);

  useEffect(() => {
    const hasActiveTransfer = transfers?.items.some((job) =>
      [
        "queued",
        "planning",
        "waitingForUser",
        "running",
        "pausing",
        "paused",
        "retrying",
        "cancelling",
      ].includes(job.status),
    );
    if (!hasActiveTransfer) return;
    const timer = window.setInterval(() => void refreshTransfers(), 1500);
    return () => window.clearInterval(timer);
  }, [transfers, refreshTransfers]);

  const openEditor = (profile?: ProfileDetail) => {
    setEditingProfile(profile ?? null);
    setEditorOpen(true);
  };

  const chooseProfile = async (profile: ProfileSummary) => {
    await selectProfile(profile.id);
    await listBuckets(profile.id);
  };

  const handleDelete = async (profile: ProfileSummary) => {
    if (
      window.confirm(
        `Delete profile “${profile.name}”? Stored credentials will be removed when no other profile uses them.`,
      )
    ) {
      await deleteProfile(profile.id);
    }
  };

  const selectedSummary = profiles.find(
    (profile) => profile.id === selectedProfileId,
  );

  const navigate = (section: string) => {
    setActiveSection(section);
    document.getElementById(section)?.scrollIntoView({ behavior: "smooth" });
  };

  return (
    <main className="min-h-screen bg-canvas text-ink">
      <div className="mx-auto flex min-h-screen max-w-[1480px] gap-6 px-6 py-6 lg:px-10">
        <aside className="hidden w-64 shrink-0 flex-col rounded-3xl border border-border bg-panel p-4 shadow-soft lg:flex">
          <div className="mb-8 flex items-center gap-3 px-3 py-2">
            <div className="grid size-10 place-items-center rounded-2xl bg-accent text-lg font-bold text-accent-foreground">
              S3
            </div>
            <div>
              <p className="font-semibold tracking-tight">S3 File Manager</p>
              <p className="text-xs text-muted">Desktop workspace</p>
            </div>
          </div>
          <nav className="space-y-1" aria-label="Primary navigation">
            {navigation.map((item) => {
              const section = item.label.toLowerCase();
              return (
                <button
                  className={`flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-sm transition ${
                    activeSection === section
                      ? "bg-accent/10 font-semibold text-accent"
                      : "text-muted hover:bg-black/[.03] hover:text-ink"
                  }`}
                  key={item.label}
                  type="button"
                  onClick={() => navigate(section)}
                >
                  <span aria-hidden="true" className="w-5 text-center">
                    {item.icon}
                  </span>
                  {item.label}
                </button>
              );
            })}
          </nav>
          <div className="mt-auto rounded-2xl bg-canvas p-4 text-xs text-muted">
            <p className="mb-2 font-semibold text-ink">Secure profile vault</p>
            <p>
              Secrets stay behind the Rust credential boundary and are never
              rendered here.
            </p>
          </div>
        </aside>

        <section className="flex min-w-0 flex-1 flex-col gap-6">
          <header className="flex flex-wrap items-center justify-between gap-4">
            <div>
              <p className="mb-1 text-xs font-semibold uppercase tracking-[0.2em] text-accent">
                Workspace
              </p>
              <h1 className="text-3xl font-semibold tracking-tight">
                Connection profiles
              </h1>
              <p className="mt-2 max-w-2xl text-sm text-muted">
                Save S3-compatible endpoints, verify access, and open a bucket
                in the explorer.
              </p>
            </div>
            <button
              className="rounded-xl bg-accent px-4 py-2.5 text-sm font-semibold text-accent-foreground shadow-sm transition hover:brightness-95"
              type="button"
              onClick={() => openEditor()}
            >
              Add profile
            </button>
          </header>

          {error && (
            <div
              className="flex items-center justify-between rounded-2xl border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-800"
              role="alert"
            >
              <span>{error}</span>
              <button
                className="ml-4 font-semibold"
                type="button"
                onClick={clearError}
              >
                Dismiss
              </button>
            </div>
          )}

          <div className="grid gap-4 sm:grid-cols-3">
            <Metric
              label="Saved profiles"
              value={loading ? "…" : String(profiles.length)}
            />
            <Metric
              label="Database schema"
              value={settings ? `v${settings.schemaVersion}` : "…"}
            />
            <Metric
              label="Implementation phase"
              value={appInfo?.phase ?? "profiles"}
            />
          </div>

          <section
            id="profiles"
            className="rounded-3xl border border-border bg-panel p-5 shadow-soft"
          >
            <div className="mb-5 flex flex-wrap items-center justify-between gap-4">
              <div>
                <h2 className="text-lg font-semibold">Profiles</h2>
                <p className="mt-1 text-sm text-muted">
                  Credentials are redacted; connection status is checked on
                  demand.
                </p>
              </div>
              <span className="rounded-full bg-canvas px-3 py-1 text-xs font-medium text-muted">
                Secure by design
              </span>
            </div>
            {loading && profiles.length === 0 ? (
              <div className="rounded-2xl border border-dashed border-border p-10 text-center text-sm text-muted">
                Loading local state…
              </div>
            ) : profiles.length === 0 ? (
              <EmptyProfiles onAdd={() => openEditor()} />
            ) : (
              <div className="grid gap-3">
                {profiles.map((profile) => (
                  <ProfileCard
                    key={profile.id}
                    profile={profile}
                    selected={profile.id === selectedProfileId}
                    busy={saving}
                    onSelect={() => void chooseProfile(profile)}
                    onEdit={() => {
                      if (activeProfile?.id === profile.id)
                        openEditor(activeProfile);
                      else
                        void selectProfile(profile.id).then(() =>
                          openEditor(
                            useAppStore.getState().activeProfile ?? undefined,
                          ),
                        );
                    }}
                    onDuplicate={() => void duplicateProfile(profile.id)}
                    onDelete={() => void handleDelete(profile)}
                  />
                ))}
              </div>
            )}
          </section>

          <ExplorerPanel
            profile={selectedSummary}
            activeProfile={activeProfile}
            buckets={buckets}
            listing={listing}
            location={location}
            loading={loading}
            onLoadBuckets={() =>
              selectedSummary && void listBuckets(selectedSummary.id)
            }
            onOpenBucket={(bucket) => {
              if (selectedSummary) {
                void openExplorer({
                  profileId: selectedSummary.id,
                  bucket,
                  prefix: activeProfile?.rootPrefix ?? "",
                });
              }
            }}
            onOpenPrefix={(prefix) => {
              if (location) void openExplorer({ ...location, prefix });
            }}
            onRefresh={() => {
              if (location) void listEntries(location);
            }}
            onSelectEntry={(entry) => {
              if (!location) return;
              void loadMetadata({
                schemaVersion: 1,
                profileId: location.profileId,
                bucket: location.bucket,
                key: entry.key,
              });
            }}
            onDeleteSelection={(entries) => {
              if (!location) return;
              for (const entry of entries) {
                const confirmation =
                  entry.kind !== "file"
                    ? window.prompt(
                        `Type DELETE ${entry.key.replace(/\/+$/, "")} to delete this prefix.`,
                      )
                    : "DELETE";
                if (!confirmation) continue;
                void startTransfer({
                  schemaVersion: 1,
                  operation: "deleteObjects",
                  profileId: location.profileId,
                  source: {
                    kind: "remote",
                    profileId: location.profileId,
                    bucket: location.bucket,
                    key: entry.key,
                  },
                  confirmation,
                  recursive: entry.kind !== "file",
                });
              }
            }}
            onNextPage={() => {
              if (location && listing?.nextToken)
                void listEntries(location, listing.nextToken);
            }}
          />

          <ObjectInspector
            metadata={metadata}
            preview={preview}
            shareLink={shareLink}
            loading={loading}
            onPreview={() => {
              if (metadata) {
                void loadPreview({
                  schemaVersion: 1,
                  profileId: metadata.profileId,
                  bucket: metadata.bucket,
                  key: metadata.key,
                });
              }
            }}
            onShare={(expiresInSeconds) => {
              if (metadata) {
                void createShare({
                  schemaVersion: 1,
                  profileId: metadata.profileId,
                  bucket: metadata.bucket,
                  key: metadata.key,
                  expiresInSeconds,
                });
              }
            }}
            onClosePreview={clearPreview}
          />

          <TransfersPanel
            id="transfers"
            profileId={selectedProfileId}
            defaultBucket={
              location?.bucket ?? activeProfile?.defaultBucket ?? ""
            }
            transfers={transfers}
            loading={transferLoading}
            onRefresh={() => void refreshTransfers()}
            onStart={startTransfer}
            onPause={pauseTransfer}
            onResume={resumeTransfer}
            onCancel={cancelTransfer}
            onRetry={retryTransfer}
            onClear={clearTransferHistory}
          />

          <SettingsPanel
            id="settings"
            settings={settings}
            onSaved={bootstrap}
          />
          <DiagnosticsPanel id="diagnostics" />

          <footer className="flex flex-wrap items-center justify-between gap-2 px-1 text-xs text-muted">
            <span>
              {appInfo?.productName ?? "S3 File Manager"}{" "}
              {appInfo?.version ?? "0.1.0"}
            </span>
            <span>Profiles and listing · schema v1</span>
          </footer>
        </section>
      </div>

      {editorOpen && (
        <ProfileEditor
          initial={
            editingProfile ? draftFromProfile(editingProfile) : emptyDraft()
          }
          profile={editingProfile}
          saving={saving}
          testing={testing}
          testResult={testResult}
          onClose={() => setEditorOpen(false)}
          onTest={(draft) => {
            const payload = { ...draft };
            if (!payload.secretAccessKey) delete payload.secretAccessKey;
            if (!payload.sessionToken) delete payload.sessionToken;
            void testProfile(payload);
          }}
          onSave={async (draft) => {
            const payload = { ...draft };
            // Empty password controls are intentionally omitted on edits so
            // Rust receives SecretInput::Unchanged rather than replacing a
            // valid credential with an empty value.
            if (!payload.sessionToken) delete payload.sessionToken;
            if (editingProfile && !payload.secretAccessKey)
              delete payload.secretAccessKey;
            const detail = await saveProfile(
              payload,
              editingProfile?.id,
              editingProfile?.revision,
            );
            if (detail) {
              setEditingProfile(detail);
              setEditorOpen(false);
            }
          }}
        />
      )}
    </main>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-2xl border border-border bg-panel p-4 shadow-soft">
      <p className="text-xs text-muted">{label}</p>
      <p className="mt-2 text-xl font-semibold capitalize">{value}</p>
    </div>
  );
}

function EmptyProfiles({ onAdd }: { onAdd: () => void }) {
  return (
    <div className="rounded-2xl border border-dashed border-border p-10 text-center">
      <div className="mx-auto mb-3 grid size-12 place-items-center rounded-2xl bg-accent/10 text-xl text-accent">
        +
      </div>
      <p className="font-medium">No profiles yet</p>
      <p className="mx-auto mt-2 max-w-md text-sm text-muted">
        Add a provider profile to verify credentials and browse buckets.
      </p>
      <button
        className="mt-5 rounded-xl bg-accent px-4 py-2 text-sm font-semibold text-accent-foreground"
        type="button"
        onClick={onAdd}
      >
        Add your first profile
      </button>
    </div>
  );
}

function ProfileCard({
  profile,
  selected,
  busy,
  onSelect,
  onEdit,
  onDuplicate,
  onDelete,
}: {
  profile: ProfileSummary;
  selected: boolean;
  busy: boolean;
  onSelect: () => void;
  onEdit: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
}) {
  return (
    <div
      className={`rounded-2xl border p-4 transition ${selected ? "border-accent bg-accent/[.04]" : "border-border"}`}
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <button
          className="min-w-0 flex-1 text-left"
          type="button"
          onClick={onSelect}
        >
          <div className="flex items-center gap-2">
            <p className="truncate font-medium">{profile.name}</p>
            {profile.favorite && (
              <span className="text-amber-500" title="Favorite">
                ★
              </span>
            )}
          </div>
          <p className="mt-1 text-xs text-muted">
            {providerLabels[profile.provider]} · {profile.region}
            {profile.defaultBucket ? ` · ${profile.defaultBucket}` : ""}
          </p>
        </button>
        <div className="flex items-center gap-2">
          <button
            className="rounded-lg border border-border px-2.5 py-1.5 text-xs font-medium hover:bg-canvas"
            type="button"
            onClick={onSelect}
            disabled={busy}
          >
            Open
          </button>
          <button
            className="rounded-lg border border-border px-2.5 py-1.5 text-xs font-medium hover:bg-canvas"
            type="button"
            onClick={onEdit}
            disabled={busy}
          >
            Edit
          </button>
          <button
            className="rounded-lg border border-border px-2.5 py-1.5 text-xs font-medium hover:bg-canvas"
            type="button"
            onClick={onDuplicate}
            disabled={busy}
          >
            Copy
          </button>
          <button
            className="rounded-lg border border-red-200 px-2.5 py-1.5 text-xs font-medium text-red-700 hover:bg-red-50"
            type="button"
            onClick={onDelete}
            disabled={busy}
          >
            Delete
          </button>
        </div>
      </div>
    </div>
  );
}

function ExplorerPanel({
  profile,
  activeProfile,
  buckets,
  listing,
  location,
  loading,
  onLoadBuckets,
  onOpenBucket,
  onOpenPrefix,
  onRefresh,
  onSelectEntry,
  onDeleteSelection,
  onNextPage,
}: {
  profile?: ProfileSummary;
  activeProfile: ProfileDetail | null;
  buckets: { name: string; creationDate?: string }[];
  listing: {
    entries: EntrySummary[];
    nextToken?: string;
    isComplete: boolean;
  } | null;
  location: ExplorerLocation | null;
  loading: boolean;
  onLoadBuckets: () => void;
  onOpenBucket: (bucket: string) => void;
  onOpenPrefix: (prefix: string) => void;
  onRefresh: () => void;
  onSelectEntry: (entry: EntrySummary) => void;
  onDeleteSelection: (entries: EntrySummary[]) => void;
  onNextPage: () => void;
}) {
  type ViewMode = "list" | "grid";
  type SortKey = "name" | "type" | "size" | "lastModified" | "storageClass";

  const [viewMode, setViewMode] = useState<ViewMode>("list");
  const [sortKey, setSortKey] = useState<SortKey>("name");
  const [sortDescending, setSortDescending] = useState(false);
  const [filter, setFilter] = useState("");
  const [selectedIds, setSelectedIds] = useState<Set<string>>(() => new Set());
  const [focusedId, setFocusedId] = useState<string | null>(null);
  const [backStack, setBackStack] = useState<ExplorerLocation[]>([]);
  const [forwardStack, setForwardStack] = useState<ExplorerLocation[]>([]);
  const scopeRef = useRef("");

  const rootPrefix = activeProfile?.rootPrefix ?? "";
  const scope = location
    ? `${location.profileId}:${location.bucket}:${location.prefix}`
    : "";

  useEffect(() => {
    setSelectedIds(new Set());
    setFocusedId(null);
    const nextScope = location
      ? `${location.profileId}:${location.bucket}`
      : "";
    const previousScope = scopeRef.current.split(":").slice(0, 2).join(":");
    if (nextScope !== previousScope) {
      setBackStack([]);
      setForwardStack([]);
    }
    scopeRef.current = scope;
  }, [location, scope]);

  const breadcrumbs = useMemo(() => {
    if (!location) return [];
    const baseParts = rootPrefix.split("/").filter(Boolean);
    const parts = location.prefix.split("/").filter(Boolean);
    const result: { label: string; prefix: string }[] = [
      { label: location.bucket, prefix: rootPrefix },
    ];
    let current = rootPrefix;
    for (const part of parts.slice(baseParts.length)) {
      current = `${current}${part}/`;
      result.push({ label: part, prefix: current });
    }
    return result;
  }, [location, rootPrefix]);

  const loadedEntries = useMemo(
    () => listing?.entries ?? [],
    [listing?.entries],
  );
  const visibleEntries = useMemo(() => {
    const needle = filter.trim().toLocaleLowerCase();
    const filtered = needle
      ? loadedEntries.filter((entry) =>
          entry.displayName.toLocaleLowerCase().includes(needle),
        )
      : loadedEntries;
    const valueFor = (entry: EntrySummary): string | number | undefined => {
      switch (sortKey) {
        case "type":
          return entry.kind === "file"
            ? (entry.contentTypeHint ?? "file")
            : "folder";
        case "size":
          return entry.size;
        case "lastModified":
          if (!entry.lastModified) return undefined;
          {
            const parsed = Date.parse(entry.lastModified);
            return Number.isFinite(parsed) ? parsed : undefined;
          }
        case "storageClass":
          return entry.storageClass;
        default:
          return entry.displayName;
      }
    };
    const compare = (left: EntrySummary, right: EntrySummary) => {
      const leftValue = valueFor(left);
      const rightValue = valueFor(right);
      const leftUnknown = leftValue === undefined || leftValue === "";
      const rightUnknown = rightValue === undefined || rightValue === "";
      if (leftUnknown !== rightUnknown) return leftUnknown ? 1 : -1;
      let result = 0;
      if (typeof leftValue === "number" && typeof rightValue === "number") {
        result = leftValue - rightValue;
      } else {
        result = String(leftValue ?? "").localeCompare(
          String(rightValue ?? ""),
          undefined,
          { sensitivity: "base", numeric: true },
        );
      }
      if (result === 0) {
        result = left.key.localeCompare(right.key, undefined, {
          sensitivity: "base",
        });
      }
      return sortDescending ? -result : result;
    };
    return [...filtered].sort(compare);
  }, [filter, loadedEntries, sortDescending, sortKey]);

  const navigatePrefix = useCallback(
    (prefix: string) => {
      if (!location || prefix === location.prefix) return;
      setBackStack((items) => [...items, location].slice(-100));
      setForwardStack([]);
      onOpenPrefix(prefix);
    },
    [location, onOpenPrefix],
  );

  const goBack = () => {
    if (!location || backStack.length === 0) return;
    const target = backStack[backStack.length - 1];
    setBackStack((items) => items.slice(0, -1));
    setForwardStack((items) => [...items, location].slice(-100));
    onOpenPrefix(target.prefix);
  };

  const goForward = () => {
    if (!location || forwardStack.length === 0) return;
    const target = forwardStack[forwardStack.length - 1];
    setForwardStack((items) => items.slice(0, -1));
    setBackStack((items) => [...items, location].slice(-100));
    onOpenPrefix(target.prefix);
  };

  const goUp = () => {
    if (!location || location.prefix === rootPrefix) return;
    const trimmed = location.prefix.replace(/\/+$/, "");
    const slash = trimmed.lastIndexOf("/");
    let parent = slash < 0 ? "" : `${trimmed.slice(0, slash + 1)}`;
    if (parent.length < rootPrefix.length || !parent.startsWith(rootPrefix)) {
      parent = rootPrefix;
    }
    navigatePrefix(parent);
  };

  const activateEntry = (entry: EntrySummary) => {
    if (entry.kind === "file") {
      onSelectEntry(entry);
      return;
    }
    navigatePrefix(entry.key.endsWith("/") ? entry.key : `${entry.key}/`);
  };

  const updateSelection = (
    entry: EntrySummary,
    event: MouseEvent<HTMLButtonElement>,
  ) => {
    const index = visibleEntries.findIndex((item) => item.id === entry.id);
    setSelectedIds((current) => {
      if (event.shiftKey && focusedId) {
        const focusedIndex = visibleEntries.findIndex(
          (item) => item.id === focusedId,
        );
        if (focusedIndex >= 0 && index >= 0) {
          const next = new Set(event.metaKey || event.ctrlKey ? current : []);
          const start = Math.min(focusedIndex, index);
          const end = Math.max(focusedIndex, index);
          for (const item of visibleEntries.slice(start, end + 1))
            next.add(item.id);
          return next;
        }
      }
      if (event.metaKey || event.ctrlKey) {
        const next = new Set(current);
        if (next.has(entry.id)) next.delete(entry.id);
        else next.add(entry.id);
        return next;
      }
      return new Set([entry.id]);
    });
    setFocusedId(entry.id);
    if (entry.kind === "file") onSelectEntry(entry);
  };

  const deleteSelection = () => {
    const selected = loadedEntries.filter((entry) => selectedIds.has(entry.id));
    if (selected.length === 0) return;
    if (
      window.confirm(
        `Delete ${selected.length} selected item${selected.length === 1 ? "" : "s"}? This cannot be undone.`,
      )
    ) {
      onDeleteSelection(selected);
      setSelectedIds(new Set());
    }
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    if (["INPUT", "SELECT", "TEXTAREA"].includes(target.tagName)) return;
    const focusedIndex = visibleEntries.findIndex(
      (entry) => entry.id === focusedId,
    );
    if (event.altKey && event.key === "ArrowLeft") {
      event.preventDefault();
      goBack();
    } else if (event.altKey && event.key === "ArrowRight") {
      event.preventDefault();
      goForward();
    } else if (event.key === "ArrowDown" || event.key === "ArrowRight") {
      event.preventDefault();
      const next =
        visibleEntries[Math.min(focusedIndex + 1, visibleEntries.length - 1)];
      if (next) setFocusedId(next.id);
    } else if (event.key === "ArrowUp" || event.key === "ArrowLeft") {
      event.preventDefault();
      const next =
        visibleEntries[Math.max(focusedIndex <= 0 ? 0 : focusedIndex - 1, 0)];
      if (next) setFocusedId(next.id);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const focused = visibleEntries.find((entry) => entry.id === focusedId);
      if (focused) activateEntry(focused);
    } else if (
      event.key === "Backspace" ||
      (event.altKey && event.key === "ArrowUp")
    ) {
      event.preventDefault();
      goUp();
    } else if (
      (event.ctrlKey || event.metaKey) &&
      event.key.toLowerCase() === "a"
    ) {
      event.preventDefault();
      setSelectedIds(new Set(visibleEntries.map((entry) => entry.id)));
      setFocusedId(visibleEntries[0]?.id ?? null);
    } else if (event.key === "Escape") {
      event.preventDefault();
      setSelectedIds(new Set());
    } else if (event.key === "Delete") {
      event.preventDefault();
      deleteSelection();
    } else if (event.key === "F5") {
      event.preventDefault();
      onRefresh();
    }
  };

  const setSort = (next: SortKey) => {
    if (next === sortKey) setSortDescending((value) => !value);
    else {
      setSortKey(next);
      setSortDescending(false);
    }
  };

  return (
    <section
      id="explorer"
      className="rounded-3xl border border-border bg-panel p-5 shadow-soft"
    >
      <div className="mb-5 flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold">Explorer</h2>
          <p className="mt-1 text-sm text-muted">
            {profile
              ? `Browse ${profile.name}`
              : "Select a profile to browse buckets."}
          </p>
        </div>
        {profile && (
          <button
            className="rounded-xl border border-border px-3 py-2 text-xs font-semibold hover:bg-canvas"
            type="button"
            onClick={onLoadBuckets}
            disabled={loading}
          >
            Refresh buckets
          </button>
        )}
      </div>
      {!profile ? (
        <div className="rounded-2xl border border-dashed border-border p-8 text-center text-sm text-muted">
          Choose a saved profile above.
        </div>
      ) : !location ? (
        <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
          {buckets.length === 0 && (
            <p className="col-span-full rounded-2xl border border-dashed border-border p-8 text-center text-sm text-muted">
              No buckets loaded. Click Refresh buckets.
            </p>
          )}
          {buckets.map((bucket) => (
            <button
              className="rounded-2xl border border-border p-4 text-left transition hover:border-accent hover:bg-accent/[.03]"
              key={bucket.name}
              type="button"
              onClick={() => onOpenBucket(bucket.name)}
            >
              <p className="font-medium">▰ {bucket.name}</p>
              <p className="mt-1 text-xs text-muted">
                {bucket.creationDate ?? "Creation date unavailable"}
              </p>
            </button>
          ))}
        </div>
      ) : (
        <div onKeyDown={handleKeyDown} tabIndex={0}>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <button
              className="rounded-lg border border-border px-2.5 py-1.5 text-xs font-semibold hover:bg-canvas disabled:opacity-40"
              type="button"
              onClick={goBack}
              disabled={backStack.length === 0 || loading}
              aria-label="Back"
            >
              ←
            </button>
            <button
              className="rounded-lg border border-border px-2.5 py-1.5 text-xs font-semibold hover:bg-canvas disabled:opacity-40"
              type="button"
              onClick={goForward}
              disabled={forwardStack.length === 0 || loading}
              aria-label="Forward"
            >
              →
            </button>
            <button
              className="rounded-lg border border-border px-2.5 py-1.5 text-xs font-semibold hover:bg-canvas disabled:opacity-40"
              type="button"
              onClick={goUp}
              disabled={location.prefix === rootPrefix || loading}
              aria-label="Up"
            >
              ↑
            </button>
            <button
              className="rounded-lg border border-border px-2.5 py-1.5 text-xs font-semibold hover:bg-canvas disabled:opacity-40"
              type="button"
              onClick={onRefresh}
              disabled={loading}
            >
              Refresh
            </button>
            <div className="min-w-[220px] flex-1">
              <input
                className="input w-full text-xs"
                value={filter}
                onChange={(event) => setFilter(event.target.value)}
                placeholder="Filter loaded items…"
                aria-label="Filter current folder"
              />
            </div>
            <select
              className="rounded-lg border border-border bg-panel px-2 py-1.5 text-xs"
              value={sortKey}
              onChange={(event) => setSort(event.target.value as SortKey)}
              aria-label="Sort loaded entries"
            >
              <option value="name">Name</option>
              <option value="type">Type</option>
              <option value="size">Size</option>
              <option value="lastModified">Last modified</option>
              <option value="storageClass">Storage class</option>
            </select>
            <button
              className="rounded-lg border border-border px-2.5 py-1.5 text-xs font-semibold hover:bg-canvas"
              type="button"
              onClick={() => setSortDescending((value) => !value)}
              aria-label="Toggle sort direction"
            >
              {sortDescending ? "↓" : "↑"}
            </button>
            <div
              className="flex rounded-lg border border-border p-0.5"
              aria-label="View mode"
            >
              <button
                className={`rounded-md px-2 py-1 text-xs ${viewMode === "list" ? "bg-accent/10 font-semibold text-accent" : "text-muted"}`}
                type="button"
                onClick={() => setViewMode("list")}
                aria-pressed={viewMode === "list"}
              >
                List
              </button>
              <button
                className={`rounded-md px-2 py-1 text-xs ${viewMode === "grid" ? "bg-accent/10 font-semibold text-accent" : "text-muted"}`}
                type="button"
                onClick={() => setViewMode("grid")}
                aria-pressed={viewMode === "grid"}
              >
                Grid
              </button>
            </div>
          </div>
          <div
            className="mb-4 flex flex-wrap items-center gap-1 text-sm"
            aria-label="Breadcrumb"
          >
            {breadcrumbs.map((crumb, index) => (
              <span
                className="flex items-center gap-1"
                key={`${crumb.prefix}-${crumb.label}`}
              >
                <button
                  className="rounded px-1.5 py-1 font-medium hover:bg-canvas"
                  type="button"
                  onClick={() => navigatePrefix(crumb.prefix)}
                >
                  {crumb.label}
                </button>
                {index < breadcrumbs.length - 1 && (
                  <span className="text-muted">/</span>
                )}
              </span>
            ))}
          </div>
          <div className="mb-3 flex flex-wrap items-center justify-between gap-2 text-xs text-muted">
            <span>
              {filter.trim()
                ? `${visibleEntries.length} of ${loadedEntries.length} loaded items`
                : `${loadedEntries.length} loaded items`}
              {listing?.nextToken ? " · Sorted loaded results" : ""}
              {selectedIds.size ? ` · ${selectedIds.size} selected` : ""}
            </span>
            {selectedIds.size > 0 && (
              <button
                className="rounded-lg border border-red-200 px-2.5 py-1.5 font-semibold text-red-700 hover:bg-red-50"
                type="button"
                onClick={deleteSelection}
              >
                Delete selected
              </button>
            )}
          </div>
          {visibleEntries.length > 0 ? (
            viewMode === "list" ? (
              <div
                className="overflow-x-auto rounded-2xl border border-border"
                role="grid"
                aria-label="Loaded objects"
              >
                <div className="grid min-w-[640px] grid-cols-[minmax(220px,1fr)_120px_110px_180px_140px] gap-3 border-b border-border bg-canvas px-4 py-2 text-[11px] font-semibold uppercase tracking-wider text-muted">
                  <button
                    className="text-left"
                    type="button"
                    onClick={() => setSort("name")}
                  >
                    Name
                  </button>
                  <button
                    className="text-left"
                    type="button"
                    onClick={() => setSort("type")}
                  >
                    Type
                  </button>
                  <button
                    className="text-right"
                    type="button"
                    onClick={() => setSort("size")}
                  >
                    Size
                  </button>
                  <button
                    className="text-left"
                    type="button"
                    onClick={() => setSort("lastModified")}
                  >
                    Last modified
                  </button>
                  <button
                    className="text-left"
                    type="button"
                    onClick={() => setSort("storageClass")}
                  >
                    Storage class
                  </button>
                </div>
                {visibleEntries.map((entry) => (
                  <button
                    className={`grid w-full min-w-[640px] grid-cols-[minmax(220px,1fr)_120px_110px_180px_140px] items-center gap-3 border-b border-border px-4 py-3 text-left text-sm last:border-b-0 ${selectedIds.has(entry.id) ? "bg-accent/10" : "hover:bg-canvas"}`}
                    key={entry.id}
                    type="button"
                    aria-selected={selectedIds.has(entry.id)}
                    onClick={(event) => updateSelection(entry, event)}
                    onDoubleClick={() => activateEntry(entry)}
                  >
                    <span className="flex min-w-0 items-center gap-3">
                      <span className="w-5 text-center" aria-hidden="true">
                        {entry.kind === "file" ? "▪" : "▰"}
                      </span>
                      <span className="truncate" title={entry.key}>
                        {entry.displayName}
                      </span>
                    </span>
                    <span className="truncate text-xs text-muted">
                      {entry.kind === "file"
                        ? (entry.contentTypeHint ?? "File")
                        : "Folder"}
                    </span>
                    <span className="text-right text-xs text-muted">
                      {entry.kind === "file" ? formatBytes(entry.size) : "—"}
                    </span>
                    <span className="truncate text-xs text-muted">
                      {entry.lastModified
                        ? new Date(entry.lastModified).toLocaleString()
                        : "—"}
                    </span>
                    <span className="truncate text-xs text-muted">
                      {entry.storageClass ?? "—"}
                    </span>
                  </button>
                ))}
              </div>
            ) : (
              <div
                className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4"
                role="grid"
                aria-label="Loaded objects"
              >
                {visibleEntries.map((entry) => (
                  <button
                    className={`rounded-2xl border p-4 text-left transition ${selectedIds.has(entry.id) ? "border-accent bg-accent/10" : "border-border hover:border-accent hover:bg-canvas"}`}
                    key={entry.id}
                    type="button"
                    aria-selected={selectedIds.has(entry.id)}
                    onClick={(event) => updateSelection(entry, event)}
                    onDoubleClick={() => activateEntry(entry)}
                  >
                    <span
                      className="mb-4 grid size-12 place-items-center rounded-xl bg-canvas text-xl"
                      aria-hidden="true"
                    >
                      {entry.kind === "file" ? "▪" : "▰"}
                    </span>
                    <span
                      className="block truncate font-medium"
                      title={entry.key}
                    >
                      {entry.displayName}
                    </span>
                    <span className="mt-1 block text-xs text-muted">
                      {entry.kind === "file"
                        ? `${entry.contentTypeHint ?? "File"} · ${formatBytes(entry.size)}`
                        : "Folder"}
                    </span>
                  </button>
                ))}
              </div>
            )
          ) : (
            <div className="rounded-2xl border border-dashed border-border p-8 text-center text-sm text-muted">
              {loading
                ? "Loading objects…"
                : filter.trim()
                  ? "No loaded items match. Clear the filter to see all loaded items."
                  : "This prefix is empty."}
            </div>
          )}
          {listing?.nextToken && (
            <div className="mt-4 text-center">
              <button
                className="rounded-xl border border-border px-4 py-2 text-xs font-semibold hover:bg-canvas"
                type="button"
                onClick={onNextPage}
                disabled={loading}
              >
                Load next page
              </button>
            </div>
          )}
        </div>
      )}
    </section>
  );
}

function formatBytes(size?: number) {
  if (size === undefined) return "—";
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  if (size < 1024 * 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} MB`;
  return `${(size / 1024 / 1024 / 1024).toFixed(1)} GB`;
}

function ObjectInspector({
  metadata,
  preview,
  shareLink,
  loading,
  onPreview,
  onShare,
  onClosePreview,
}: {
  metadata: ObjectMetadata | null;
  preview: PreviewResult | null;
  shareLink: ShareLink | null;
  loading: boolean;
  onPreview: () => void;
  onShare: (expiresInSeconds: number) => void;
  onClosePreview: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const [shareExpiry, setShareExpiry] = useState("3600");
  useEffect(() => setCopied(false), [shareLink?.url]);
  if (!metadata) {
    return (
      <section className="rounded-3xl border border-border bg-panel p-5 shadow-soft">
        <h2 className="text-lg font-semibold">Object details</h2>
        <p className="mt-2 text-sm text-muted">
          Select a file in the explorer to inspect metadata, preview text, or
          create a temporary share link.
        </p>
      </section>
    );
  }

  const copyShareLink = async () => {
    if (!shareLink) return;
    if (!navigator.clipboard) return;
    try {
      await navigator.clipboard.writeText(shareLink.url);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1_500);
    } catch {
      setCopied(false);
    }
  };
  const rows: [string, string | undefined][] = [
    ["Type", metadata.contentType],
    [
      "Size",
      metadata.size === undefined ? "Unknown" : formatBytes(metadata.size),
    ],
    ["ETag", metadata.etag],
    ["Version", metadata.versionId],
    ["Last modified", metadata.lastModified],
    ["Storage class", metadata.storageClass],
    ["Encryption", metadata.encryption],
    ["Disposition", metadata.contentDisposition],
    ["Cache control", metadata.cacheControl],
    ["Encoding", metadata.contentEncoding],
  ];
  return (
    <section className="rounded-3xl border border-border bg-panel p-5 shadow-soft">
      <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h2 className="text-lg font-semibold">Object details</h2>
          <p className="mt-1 truncate text-sm text-muted" title={metadata.key}>
            {metadata.bucket}/{metadata.key}
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <button
            className="rounded-xl border border-border px-3 py-2 text-xs font-semibold hover:bg-canvas disabled:opacity-50"
            type="button"
            onClick={onPreview}
            disabled={loading || !metadata.previewSupported}
            title={metadata.previewReason}
          >
            {loading ? "Loading…" : "Preview"}
          </button>
          <div className="flex gap-2">
            <select
              className="rounded-xl border border-border bg-panel px-2 py-2 text-xs"
              value={shareExpiry}
              onChange={(event) => setShareExpiry(event.target.value)}
              aria-label="Share link expiry"
            >
              <option value="300">5 minutes</option>
              <option value="900">15 minutes</option>
              <option value="3600">1 hour</option>
              <option value="21600">6 hours</option>
              <option value="86400">24 hours</option>
              <option value="604800">7 days</option>
            </select>
            <button
              className="rounded-xl bg-accent px-3 py-2 text-xs font-semibold text-accent-foreground hover:brightness-95 disabled:opacity-50"
              type="button"
              onClick={() => onShare(Number(shareExpiry))}
              disabled={loading}
            >
              Create share link
            </button>
          </div>
        </div>
      </div>
      <div className="grid gap-x-6 gap-y-2 sm:grid-cols-2">
        {rows.map(([label, value]) => (
          <div
            className="flex min-w-0 justify-between gap-3 text-sm"
            key={label}
          >
            <span className="text-muted">{label}</span>
            <span className="truncate text-right" title={value ?? undefined}>
              {value ?? "Unknown"}
            </span>
          </div>
        ))}
      </div>
      {Object.keys(metadata.userMetadata).length > 0 && (
        <div className="mt-4 border-t border-border pt-4">
          <p className="mb-2 text-xs font-semibold uppercase tracking-wider text-muted">
            User metadata
          </p>
          <div className="grid gap-1 text-sm sm:grid-cols-2">
            {Object.entries(metadata.userMetadata).map(([key, value]) => (
              <div className="flex min-w-0 justify-between gap-3" key={key}>
                <span className="truncate text-muted">{key}</span>
                <span className="truncate text-right" title={value}>
                  {value}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
      {preview && (
        <div className="mt-4 border-t border-border pt-4">
          <div className="mb-2 flex items-center justify-between gap-3">
            <p className="text-xs font-semibold uppercase tracking-wider text-muted">
              Preview · {formatBytes(preview.bytesRead)}
            </p>
            <div className="flex items-center gap-2 text-xs text-muted">
              <span>
                {preview.truncated
                  ? "Preview truncated"
                  : preview.expiresAt
                    ? `Expires ${new Date(preview.expiresAt).toLocaleTimeString()}`
                    : ""}
              </span>
              <button
                className="font-semibold text-ink hover:text-accent"
                type="button"
                onClick={onClosePreview}
              >
                Close
              </button>
            </div>
          </div>
          {preview.previewKind === "text" ? (
            <pre className="max-h-96 overflow-auto whitespace-pre-wrap break-words rounded-xl bg-canvas p-3 text-xs leading-5">
              {preview.text}
            </pre>
          ) : preview.url && preview.previewKind === "image" ? (
            <img
              className="max-h-96 max-w-full rounded-xl bg-canvas object-contain"
              src={preview.url}
              alt={metadata.key}
            />
          ) : preview.url && preview.previewKind === "audio" ? (
            <audio className="w-full" controls src={preview.url} />
          ) : preview.url && preview.previewKind === "video" ? (
            <video
              className="max-h-96 w-full rounded-xl bg-ink"
              controls
              src={preview.url}
            />
          ) : preview.url && preview.previewKind === "pdf" ? (
            <iframe
              className="h-96 w-full rounded-xl border border-border"
              src={preview.url}
              title={`PDF preview: ${metadata.key}`}
              sandbox=""
            />
          ) : (
            <p className="rounded-xl bg-canvas p-3 text-sm text-muted">
              Preview handle expired. Select Preview to request a new one.
            </p>
          )}
        </div>
      )}
      {shareLink && (
        <div className="mt-4 border-t border-border pt-4">
          <p className="mb-2 text-xs font-semibold uppercase tracking-wider text-muted">
            Temporary share link · {shareLink.expiresInSeconds}s
          </p>
          <div className="flex gap-2">
            <input
              className="input min-w-0 flex-1 text-xs"
              value={shareLink.url}
              readOnly
              aria-label="Temporary share link"
            />
            <button
              className="rounded-xl border border-border px-3 py-2 text-xs font-semibold hover:bg-canvas"
              type="button"
              onClick={() => void copyShareLink()}
            >
              {copied ? "Copied" : "Copy"}
            </button>
          </div>
          <p className="mt-1 text-xs text-muted">
            Expires {new Date(shareLink.expiresAt).toLocaleString()}
          </p>
        </div>
      )}
    </section>
  );
}

function TransfersPanel({
  id,
  profileId,
  defaultBucket,
  transfers,
  loading,
  onRefresh,
  onStart,
  onPause,
  onResume,
  onCancel,
  onRetry,
  onClear,
}: {
  id: string;
  profileId: string | null;
  defaultBucket: string;
  transfers: TransferHistoryPage | null;
  loading: boolean;
  onRefresh: () => void;
  onStart: (request: StartTransferRequest) => Promise<TransferJob | null>;
  onPause: (id: string) => Promise<void>;
  onResume: (id: string) => Promise<void>;
  onCancel: (id: string) => Promise<void>;
  onRetry: (id: string) => Promise<void>;
  onClear: () => Promise<void>;
}) {
  const [operation, setOperation] = useState<TransferOperation>("uploadFile");
  const [bucket, setBucket] = useState(defaultBucket);
  const [sourceKey, setSourceKey] = useState("");
  const [destinationKey, setDestinationKey] = useState("");
  const [localPath, setLocalPath] = useState("");
  const [collisionPolicy, setCollisionPolicy] =
    useState<CollisionPolicy>("ask");
  const [formError, setFormError] = useState("");

  useEffect(() => {
    if (defaultBucket && !bucket) setBucket(defaultBucket);
  }, [bucket, defaultBucket]);

  const isUpload =
    operation === "uploadFile" || operation === "uploadDirectory";
  const isDownload =
    operation === "downloadFile" || operation === "downloadPrefix";
  const isDelete = operation === "deleteObjects";

  const start = async () => {
    setFormError("");
    if (!profileId) {
      setFormError("Select a profile first.");
      return;
    }
    if (!bucket.trim()) {
      setFormError("A bucket is required.");
      return;
    }
    if (isUpload && !localPath.trim()) {
      setFormError("Choose a local source path.");
      return;
    }
    if (!isUpload && !sourceKey.trim()) {
      setFormError("A source key or prefix is required.");
      return;
    }
    if (!isDelete && !isDownload && !destinationKey.trim()) {
      setFormError("A destination key or prefix is required.");
      return;
    }
    if (isDownload && !localPath.trim()) {
      setFormError("Choose a local destination path.");
      return;
    }
    if (
      isDelete &&
      !window.confirm("Delete the selected object permanently?")
    ) {
      return;
    }
    let confirmation = isDelete ? "DELETE" : undefined;
    if (isDelete && sourceKey.trim().endsWith("/")) {
      const prefix = sourceKey.trim().replace(/\/+$/, "");
      const typed = window.prompt(
        `For a large recursive delete, type DELETE ${prefix} to continue. Otherwise click Cancel.`,
      );
      if (typed) confirmation = typed;
    }
    const remote = (key: string): TransferEndpoint => ({
      kind: "remote",
      profileId,
      bucket: bucket.trim(),
      key: key.trim(),
    });
    let source: TransferEndpoint;
    let destination: TransferEndpoint | undefined;
    if (isUpload) {
      source = { kind: "local", path: localPath.trim() };
      destination = remote(destinationKey);
    } else if (isDownload) {
      source = remote(sourceKey);
      destination = { kind: "local", path: localPath.trim() };
    } else {
      source = remote(sourceKey);
      destination = isDelete ? undefined : remote(destinationKey);
    }
    await onStart({
      schemaVersion: 1,
      operation,
      profileId,
      source,
      destination,
      collisionPolicy,
      confirmation,
      recursive: isDelete && sourceKey.trim().endsWith("/"),
    });
  };

  return (
    <section
      id={id}
      className="rounded-3xl border border-border bg-panel p-5 shadow-soft"
    >
      <div className="mb-5 flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold">Transfers</h2>
          <p className="mt-1 text-sm text-muted">
            Queue uploads, downloads, copy/move jobs, and inspect their state.
          </p>
        </div>
        <div className="flex gap-2">
          <button
            className="rounded-xl border border-border px-3 py-2 text-xs font-semibold hover:bg-canvas"
            type="button"
            onClick={onRefresh}
            disabled={loading}
          >
            Refresh
          </button>
          <button
            className="rounded-xl border border-red-200 px-3 py-2 text-xs font-semibold text-red-700 hover:bg-red-50"
            type="button"
            onClick={() => void onClear()}
            disabled={loading}
          >
            Clear history
          </button>
        </div>
      </div>
      <div className="grid gap-3 rounded-2xl border border-border bg-canvas p-4 md:grid-cols-2 lg:grid-cols-4">
        <label className="text-xs font-medium">
          Operation
          <select
            className="input mt-1"
            value={operation}
            onChange={(event) =>
              setOperation(event.target.value as TransferOperation)
            }
          >
            <option value="uploadFile">Upload file</option>
            <option value="uploadDirectory">Upload folder</option>
            <option value="downloadFile">Download file</option>
            <option value="downloadPrefix">Download prefix</option>
            <option value="copyObject">Copy object</option>
            <option value="moveObject">Move / rename object</option>
            <option value="copyPrefix">Copy prefix</option>
            <option value="movePrefix">Move / rename prefix</option>
            <option value="deleteObjects">Delete object</option>
          </select>
        </label>
        <label className="text-xs font-medium">
          Bucket
          <input
            className="input mt-1"
            value={bucket}
            onChange={(event) => setBucket(event.target.value)}
            placeholder="bucket-name"
          />
        </label>
        <label className="text-xs font-medium">
          {isUpload ? "Local source path" : "Source key"}
          <input
            className="input mt-1"
            value={isUpload ? localPath : sourceKey}
            onChange={(event) =>
              isUpload
                ? setLocalPath(event.target.value)
                : setSourceKey(event.target.value)
            }
            placeholder={isUpload ? "C:\\files\\report.txt" : "folder/file.txt"}
          />
        </label>
        {!isDelete && (
          <label className="text-xs font-medium">
            {isDownload ? "Local destination path" : "Destination key"}
            <input
              className="input mt-1"
              value={isDownload ? localPath : destinationKey}
              onChange={(event) =>
                isDownload
                  ? setLocalPath(event.target.value)
                  : setDestinationKey(event.target.value)
              }
              placeholder={isDownload ? "C:\\Downloads" : "archive/file.txt"}
            />
          </label>
        )}
        <label className="text-xs font-medium">
          Collision policy
          <select
            className="input mt-1"
            value={collisionPolicy}
            onChange={(event) =>
              setCollisionPolicy(event.target.value as CollisionPolicy)
            }
          >
            <option value="ask">Ask / stop safely</option>
            <option value="replace">Replace</option>
            <option value="skip">Skip</option>
            <option value="fail">Fail</option>
          </select>
        </label>
        <div className="flex items-end">
          <button
            className="w-full rounded-xl bg-accent px-3 py-2.5 text-xs font-semibold text-accent-foreground hover:brightness-95 disabled:opacity-50"
            type="button"
            onClick={() => void start()}
            disabled={loading || !profileId}
          >
            {loading ? "Starting…" : "Start transfer"}
          </button>
        </div>
      </div>
      {formError && (
        <p className="mt-3 rounded-xl border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-800">
          {formError}
        </p>
      )}
      {!profileId ? (
        <p className="mt-4 rounded-2xl border border-dashed border-border p-6 text-center text-sm text-muted">
          Select a profile before creating a transfer.
        </p>
      ) : transfers?.items.length ? (
        <div className="mt-4 divide-y divide-border rounded-2xl border border-border">
          {transfers.items.map((job) => (
            <TransferRow
              key={job.id}
              job={job}
              onPause={onPause}
              onResume={onResume}
              onCancel={onCancel}
              onRetry={onRetry}
            />
          ))}
        </div>
      ) : (
        <p className="mt-4 rounded-2xl border border-dashed border-border p-6 text-center text-sm text-muted">
          No transfer jobs yet.
        </p>
      )}
    </section>
  );
}

function TransferRow({
  job,
  onPause,
  onResume,
  onCancel,
  onRetry,
}: {
  job: TransferHistoryPage["items"][number];
  onPause: (id: string) => Promise<void>;
  onResume: (id: string) => Promise<void>;
  onCancel: (id: string) => Promise<void>;
  onRetry: (id: string) => Promise<void>;
}) {
  const isActive = ![
    "completed",
    "completedWithWarnings",
    "failed",
    "cancelled",
    "interrupted",
  ].includes(job.status);
  const pauseSupported = [
    "uploadDirectory",
    "downloadPrefix",
    "copyPrefix",
    "movePrefix",
  ].includes(job.operation);
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-3 text-sm">
      <div>
        <p className="font-medium">
          {job.operation} · {job.status}
        </p>
        <p className="text-xs text-muted">
          {formatBytes(job.transferredBytes)}
          {job.totalBytes === undefined
            ? ""
            : ` / ${formatBytes(job.totalBytes)}`}
          {job.totalItems === undefined
            ? ""
            : ` · ${job.completedItems}/${job.totalItems} items`}
        </p>
      </div>
      <div className="flex flex-wrap gap-2">
        {isActive && pauseSupported && job.status === "running" && (
          <button
            className="rounded-lg border border-border px-2 py-1 text-xs"
            type="button"
            onClick={() => void onPause(job.id)}
          >
            Pause
          </button>
        )}
        {isActive &&
          pauseSupported &&
          (job.status === "paused" || job.status === "pausing") && (
            <button
              className="rounded-lg border border-border px-2 py-1 text-xs"
              type="button"
              onClick={() => void onResume(job.id)}
            >
              Resume
            </button>
          )}
        {isActive && (
          <button
            className="rounded-lg border border-red-200 px-2 py-1 text-xs text-red-700"
            type="button"
            onClick={() => void onCancel(job.id)}
          >
            Cancel
          </button>
        )}
        {!isActive && job.status !== "completed" && (
          <button
            className="rounded-lg border border-border px-2 py-1 text-xs"
            type="button"
            onClick={() => void onRetry(job.id)}
          >
            Retry
          </button>
        )}
      </div>
    </div>
  );
}

function SettingsPanel({
  id,
  settings,
  onSaved,
}: {
  id: string;
  settings: SettingsSnapshot | null;
  onSaved: () => Promise<void>;
}) {
  const [concurrentJobs, setConcurrentJobs] = useState("4");
  const [partConcurrency, setPartConcurrency] = useState("4");
  const [keepPartial, setKeepPartial] = useState(false);
  const [message, setMessage] = useState("");

  useEffect(() => {
    if (!settings) return;
    setConcurrentJobs(
      String(settings.concurrentJobs ?? settings.transferConcurrency ?? 4),
    );
    setPartConcurrency(
      String(settings.perJobPartConcurrency ?? settings.partConcurrency ?? 4),
    );
    setKeepPartial(Boolean(settings.keepPartialDownloads));
  }, [settings]);

  const save = async () => {
    try {
      await commands.updateSettings({
        schemaVersion: 1,
        concurrentJobs: Number(concurrentJobs),
        perJobPartConcurrency: Number(partConcurrency),
        keepPartialDownloads: keepPartial,
      });
      await onSaved();
      setMessage("Settings saved");
    } catch (error) {
      setMessage(formatCommandError(error));
    }
  };

  return (
    <section
      id={id}
      className="rounded-3xl border border-border bg-panel p-5 shadow-soft"
    >
      <div className="mb-5 flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold">Settings</h2>
          <p className="mt-1 text-sm text-muted">
            Transfer limits and safe recovery defaults are persisted locally.
          </p>
        </div>
        <button
          className="rounded-xl border border-border px-3 py-2 text-xs font-semibold"
          type="button"
          onClick={() => void save()}
        >
          Save settings
        </button>
      </div>
      <div className="grid gap-4 sm:grid-cols-3">
        <label className="text-sm">
          Concurrent jobs
          <input
            className="input mt-1"
            type="number"
            min={1}
            max={16}
            value={concurrentJobs}
            onChange={(event) => setConcurrentJobs(event.target.value)}
          />
        </label>
        <label className="text-sm">
          Parts per job
          <input
            className="input mt-1"
            type="number"
            min={1}
            max={16}
            value={partConcurrency}
            onChange={(event) => setPartConcurrency(event.target.value)}
          />
        </label>
        <label className="flex items-center gap-2 self-end text-sm">
          <input
            type="checkbox"
            checked={keepPartial}
            onChange={(event) => setKeepPartial(event.target.checked)}
          />{" "}
          Keep partial downloads
        </label>
      </div>
      {message && (
        <p className="mt-3 text-xs text-muted" role="status">
          {message}
        </p>
      )}
    </section>
  );
}

function DiagnosticsPanel({ id }: { id: string }) {
  const [destination, setDestination] = useState("");
  const [message, setMessage] = useState("");
  const exportDiagnostics = async () => {
    try {
      const result = await commands.exportDiagnostics({
        schemaVersion: 1,
        destinationPath: destination,
      });
      setMessage(`Exported redacted diagnostics to ${result.path}`);
    } catch (error) {
      setMessage(formatCommandError(error));
    }
  };
  const showLogDirectory = async () => {
    try {
      const result = await commands.openLogDirectory();
      setMessage(`Log directory: ${result.path}`);
    } catch (error) {
      setMessage(formatCommandError(error));
    }
  };
  return (
    <section
      id={id}
      className="rounded-3xl border border-border bg-panel p-5 shadow-soft"
    >
      <div className="mb-5 flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold">Diagnostics & updates</h2>
          <p className="mt-1 text-sm text-muted">
            Exports contain redacted configuration and recent operational logs
            only.
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <button
            className="rounded-xl border border-border px-3 py-2 text-xs font-semibold"
            type="button"
            onClick={() => void showLogDirectory()}
          >
            Log folder
          </button>
          <button
            className="rounded-xl border border-border px-3 py-2 text-xs font-semibold"
            type="button"
            onClick={() =>
              void commands
                .clearLogs()
                .then(() => setMessage("Logs cleared"))
                .catch((error) => setMessage(formatCommandError(error)))
            }
          >
            Clear logs
          </button>
          <button
            className="rounded-xl border border-border px-3 py-2 text-xs font-semibold"
            type="button"
            onClick={() =>
              void commands
                .checkForUpdates()
                .then((result) => setMessage(result.message))
                .catch((error) => setMessage(formatCommandError(error)))
            }
          >
            Check updates
          </button>
        </div>
      </div>
      <div className="flex flex-wrap gap-2">
        <input
          className="input min-w-[18rem] flex-1"
          value={destination}
          onChange={(event) => setDestination(event.target.value)}
          placeholder="C:\\Users\\You\\Desktop\\s3fm-diagnostics.json"
          aria-label="Diagnostics export path"
        />
        <button
          className="rounded-xl bg-accent px-3 py-2 text-xs font-semibold text-accent-foreground disabled:opacity-50"
          type="button"
          disabled={!destination.trim()}
          onClick={() => void exportDiagnostics()}
        >
          Export redacted diagnostics
        </button>
      </div>
      {message && (
        <p className="mt-3 text-xs text-muted" role="status">
          {message}
        </p>
      )}
    </section>
  );
}

function ProfileEditor({
  initial,
  profile,
  saving,
  testing,
  testResult,
  onClose,
  onTest,
  onSave,
}: {
  initial: ProfileDraft;
  profile: ProfileDetail | null;
  saving: boolean;
  testing: boolean;
  testResult: { success: boolean; message: string; latencyMs: number } | null;
  onClose: () => void;
  onTest: (draft: ProfileDraft) => void;
  onSave: (draft: ProfileDraft) => Promise<void>;
}) {
  const [draft, setDraft] = useState<ProfileDraft>(initial);
  const update = <K extends keyof ProfileDraft>(
    key: K,
    value: ProfileDraft[K],
  ) => setDraft((current) => ({ ...current, [key]: value }));
  const isCustomEndpoint =
    draft.provider === "customS3" || draft.provider === "minio";
  const clearSecretFields = () =>
    setDraft((current) => ({
      ...current,
      secretAccessKey: "",
      sessionToken: "",
    }));
  const setProvider = (provider: ProviderType) => {
    const defaults: Record<ProviderType, Partial<ProfileDraft>> = {
      awsS3: {
        region: "",
        endpoint: "",
        addressingStyle: "virtualHosted",
        allowInsecureHttp: false,
      },
      cloudflareR2: {
        region: "auto",
        endpoint: "",
        addressingStyle: "virtualHosted",
        allowInsecureHttp: false,
      },
      minio: {
        region: "us-east-1",
        addressingStyle: "path",
        // HTTP is opt-in even for local MinIO; the checkbox below makes the
        // downgrade explicit for development environments.
        allowInsecureHttp: false,
      },
      wasabi: {
        region: "us-east-1",
        endpoint: "",
        addressingStyle: "virtualHosted",
        allowInsecureHttp: false,
      },
      customS3: { region: "us-east-1", addressingStyle: "path" },
    };
    setDraft((current) => ({ ...current, provider, ...defaults[provider] }));
  };

  return (
    <div
      className="fixed inset-0 z-20 grid place-items-center bg-ink/30 p-4"
      role="dialog"
      aria-modal="true"
      aria-labelledby="profile-editor-title"
    >
      <form
        className="max-h-[92vh] w-full max-w-3xl overflow-y-auto rounded-3xl border border-border bg-panel p-6 shadow-2xl"
        onSubmit={(event) => {
          event.preventDefault();
          const payload = { ...draft };
          clearSecretFields();
          void onSave(payload);
        }}
      >
        <div className="mb-6 flex items-start justify-between gap-4">
          <div>
            <p className="text-xs font-semibold uppercase tracking-[0.2em] text-accent">
              Profile editor
            </p>
            <h2
              className="mt-1 text-2xl font-semibold"
              id="profile-editor-title"
            >
              {profile ? "Edit connection profile" : "Add connection profile"}
            </h2>
            <p className="mt-1 text-sm text-muted">
              Provider defaults can be adjusted before saving.
            </p>
          </div>
          <button
            className="rounded-lg px-2 py-1 text-xl text-muted hover:bg-canvas"
            type="button"
            onClick={onClose}
            aria-label="Close"
          >
            ×
          </button>
        </div>
        <div className="grid gap-5 sm:grid-cols-2">
          <Field label="Display name" required>
            <input
              className="input"
              value={draft.name}
              onChange={(event) => update("name", event.target.value)}
              required
              maxLength={80}
            />
          </Field>
          <Field label="Provider">
            <select
              className="input"
              value={draft.provider}
              onChange={(event) =>
                setProvider(event.target.value as ProviderType)
              }
            >
              {Object.entries(providerLabels).map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </select>
          </Field>
          {draft.provider === "cloudflareR2" && (
            <Field label="Account ID" hint="Used to derive the R2 endpoint">
              <input
                className="input"
                value={draft.accountId}
                onChange={(event) => update("accountId", event.target.value)}
                placeholder="Account identifier"
              />
            </Field>
          )}
          <Field
            label="Region"
            required
            hint={
              draft.provider === "cloudflareR2" ? "R2 uses auto" : undefined
            }
          >
            <input
              className="input"
              value={draft.region}
              onChange={(event) => update("region", event.target.value)}
              required
            />
          </Field>
          <Field
            label="Endpoint"
            hint={
              isCustomEndpoint
                ? "HTTPS is recommended"
                : "Managed by the selected provider"
            }
          >
            <input
              className="input"
              value={draft.endpoint}
              disabled={!isCustomEndpoint}
              onChange={(event) => update("endpoint", event.target.value)}
              placeholder={
                draft.provider === "minio"
                  ? "http://localhost:9000"
                  : "https://…"
              }
            />
          </Field>
          <Field label="Addressing style">
            <select
              className="input"
              value={draft.addressingStyle}
              onChange={(event) =>
                update("addressingStyle", event.target.value as AddressingStyle)
              }
            >
              <option value="virtualHosted">Virtual-hosted</option>
              <option value="path">Path-style</option>
            </select>
          </Field>
          <Field label="Credential mode">
            <select
              className="input"
              value={draft.credentialMode}
              onChange={(event) =>
                update("credentialMode", event.target.value as CredentialMode)
              }
            >
              <option value="static">Static access key</option>
              <option value="temporarySession">Temporary session</option>
            </select>
          </Field>
          <Field label="Access key ID" required={!profile}>
            <input
              className="input"
              value={draft.accessKeyId}
              onChange={(event) => update("accessKeyId", event.target.value)}
              autoComplete="off"
              placeholder={
                profile ? "Leave blank to keep current" : "Access key ID"
              }
            />
          </Field>
          <Field
            label={`Secret access key${profile ? " (optional change)" : ""}`}
            required={!profile}
          >
            <input
              className="input"
              type="password"
              value={draft.secretAccessKey}
              onChange={(event) =>
                update("secretAccessKey", event.target.value)
              }
              autoComplete="new-password"
              placeholder={
                profile ? "Leave blank to keep current" : "Secret access key"
              }
            />
          </Field>
          <Field
            label="Session token"
            hint="Only required for temporary credentials"
          >
            <input
              className="input"
              type="password"
              value={draft.sessionToken}
              onChange={(event) => update("sessionToken", event.target.value)}
              autoComplete="new-password"
            />
          </Field>
          <Field label="Default bucket">
            <input
              className="input"
              value={draft.defaultBucket}
              onChange={(event) => update("defaultBucket", event.target.value)}
              placeholder="Optional"
            />
          </Field>
          <Field label="Root prefix" hint="Navigation cannot leave this prefix">
            <input
              className="input"
              value={draft.rootPrefix}
              onChange={(event) => update("rootPrefix", event.target.value)}
              placeholder="team/"
            />
          </Field>
        </div>
        <div className="mt-5 flex flex-wrap gap-5 text-sm">
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={draft.favorite}
              onChange={(event) => update("favorite", event.target.checked)}
            />{" "}
            Favorite profile
          </label>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={draft.allowInsecureHttp}
              disabled={!isCustomEndpoint}
              onChange={(event) =>
                update(
                  "allowInsecureHttp",
                  event.target.checked &&
                    window.confirm(
                      "HTTP sends credentials and object data without transport encryption. Continue only for a trusted local endpoint?",
                    ),
                )
              }
            />{" "}
            Allow HTTP (local providers only)
          </label>
        </div>
        {testResult && (
          <div
            className={`mt-5 rounded-2xl border px-4 py-3 text-sm ${testResult.success ? "border-emerald-200 bg-emerald-50 text-emerald-800" : "border-amber-200 bg-amber-50 text-amber-800"}`}
            role="status"
          >
            <p className="font-semibold">
              {testResult.success
                ? "Connection successful"
                : "Connection check failed"}
            </p>
            <p className="mt-1">
              {testResult.message} · {testResult.latencyMs} ms
            </p>
          </div>
        )}
        <div className="mt-7 flex flex-wrap justify-end gap-3">
          <button
            className="rounded-xl border border-border px-4 py-2.5 text-sm font-semibold hover:bg-canvas"
            type="button"
            onClick={() => {
              const payload = { ...draft };
              clearSecretFields();
              onTest(payload);
            }}
            disabled={saving || testing}
          >
            {testing ? "Testing…" : "Test connection"}
          </button>
          <button
            className="rounded-xl border border-border px-4 py-2.5 text-sm font-semibold hover:bg-canvas"
            type="button"
            onClick={onClose}
            disabled={saving}
          >
            Cancel
          </button>
          <button
            className="rounded-xl bg-accent px-4 py-2.5 text-sm font-semibold text-accent-foreground hover:brightness-95"
            type="submit"
            disabled={saving}
          >
            {saving ? "Saving…" : profile ? "Save changes" : "Save profile"}
          </button>
        </div>
      </form>
    </div>
  );
}

function Field({
  label,
  hint,
  required,
  children,
}: {
  label: string;
  hint?: string;
  required?: boolean;
  children: ReactNode;
}) {
  return (
    <label className="block">
      <span className="mb-1.5 block text-sm font-medium">
        {label}
        {required && <span className="ml-1 text-red-600">*</span>}
      </span>
      {children}
      {hint && <span className="mt-1 block text-xs text-muted">{hint}</span>}
    </label>
  );
}

export default App;
