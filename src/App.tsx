import { useEffect } from "react";
import { useAppStore } from "./stores/appStore";

const navigation = [
  { label: "Profiles", icon: "◉", active: true },
  { label: "Explorer", icon: "▦", active: false },
  { label: "Transfers", icon: "⇄", active: false },
  { label: "Settings", icon: "⚙", active: false },
];

function App() {
  const { appInfo, profiles, settings, loading, error, bootstrap } =
    useAppStore();

  useEffect(() => {
    void bootstrap();
  }, [bootstrap]);

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
            {navigation.map((item) => (
              <button
                className={`flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-sm transition ${
                  item.active
                    ? "bg-accent/10 font-semibold text-accent"
                    : "text-muted hover:bg-black/[.03] hover:text-ink"
                }`}
                key={item.label}
                type="button"
                disabled={!item.active}
              >
                <span aria-hidden="true" className="w-5 text-center">
                  {item.icon}
                </span>
                {item.label}
                {!item.active && (
                  <span className="ml-auto text-[10px] uppercase tracking-wider text-muted">
                    Soon
                  </span>
                )}
              </button>
            ))}
          </nav>
          <div className="mt-auto rounded-2xl bg-canvas p-4 text-xs text-muted">
            <p className="mb-2 font-semibold text-ink">Foundation ready</p>
            <p>
              Secrets stay in the Rust credential boundary. Transfer workflows
              arrive in the next phases.
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
                Keep your S3-compatible endpoints organized before opening a
                bucket.
              </p>
            </div>
            <button
              className="rounded-xl bg-accent px-4 py-2.5 text-sm font-semibold text-accent-foreground shadow-sm transition hover:brightness-95"
              type="button"
              disabled
            >
              Add profile{" "}
              <span className="ml-2 text-xs opacity-70">Phase 1</span>
            </button>
          </header>

          {error && (
            <div
              className="rounded-2xl border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-800"
              role="alert"
            >
              {error}
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
              value={appInfo?.phase ?? "…"}
            />
          </div>

          <section className="flex-1 rounded-3xl border border-border bg-panel p-5 shadow-soft">
            <div className="mb-5 flex items-center justify-between gap-4">
              <div>
                <h2 className="text-lg font-semibold">Profiles</h2>
                <p className="mt-1 text-sm text-muted">
                  No credentials are displayed in this view.
                </p>
              </div>
              <span className="rounded-full bg-canvas px-3 py-1 text-xs font-medium text-muted">
                Secure by design
              </span>
            </div>
            {loading ? (
              <div className="rounded-2xl border border-dashed border-border p-10 text-center text-sm text-muted">
                Loading local state…
              </div>
            ) : profiles.length === 0 ? (
              <div className="rounded-2xl border border-dashed border-border p-10 text-center">
                <div className="mx-auto mb-3 grid size-12 place-items-center rounded-2xl bg-accent/10 text-xl text-accent">
                  +
                </div>
                <p className="font-medium">No profiles yet</p>
                <p className="mx-auto mt-2 max-w-md text-sm text-muted">
                  Profile CRUD is the next implementation phase. The local
                  database and typed command boundary are ready.
                </p>
              </div>
            ) : (
              <div className="grid gap-3">
                {profiles.map((profile) => (
                  <ProfileCard key={profile.id} profile={profile} />
                ))}
              </div>
            )}
          </section>

          <footer className="flex flex-wrap items-center justify-between gap-2 px-1 text-xs text-muted">
            <span>
              {appInfo?.productName ?? "S3 File Manager"}{" "}
              {appInfo?.version ?? "0.1.0"}
            </span>
            <span>Phase 0 foundation</span>
          </footer>
        </section>
      </div>
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

function ProfileCard({
  profile,
}: {
  profile: { name: string; provider: string; region: string };
}) {
  return (
    <div className="flex items-center justify-between rounded-2xl border border-border p-4">
      <div>
        <p className="font-medium">{profile.name}</p>
        <p className="mt-1 text-xs text-muted">
          {profile.provider} · {profile.region}
        </p>
      </div>
      <span className="text-xs text-muted">Ready</span>
    </div>
  );
}

export default App;
