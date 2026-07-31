import { useCallback, useEffect, useMemo, useRef, useState } from "react";
// The bundle icon itself, not a copy of it: this is the file Tauri stamps onto
// the executable and the launcher, so the mark in the window cannot drift from
// the one on the desktop. Vite fingerprints and inlines it at build time.
import logo from "../src-tauri/icons/128x128.png";
import * as api from "./api";
import { usePersisted, usePolled, useTheme } from "./hooks";
import type { ClusterInfo, RuntimeInfo, RuntimeKind, SystemSummary } from "./types";
import { Badge, Button, Dot, Spinner, ThemeToggle, ToastProvider, useToast } from "./ui";
import { RELEASES_URL, useUpdates, type UpdateState } from "./updates";
import { formatBytes } from "./util";
import Containers from "./views/Containers";
import Dashboard from "./views/Dashboard";
import Images from "./views/Images";
import Networks from "./views/Networks";
import Volumes from "./views/Volumes";
import Activity from "./views/Activity";
import Compose from "./views/Compose";
import Kubernetes from "./views/Kubernetes";

type ViewId =
  | "dashboard"
  | "containers"
  | "images"
  | "networks"
  | "volumes"
  | "compose"
  | "kubernetes"
  | "activity";

const NAV: { id: ViewId; label: string; icon: string }[] = [
  { id: "dashboard", label: "Dashboard", icon: "▦" },
  { id: "containers", label: "Containers", icon: "▣" },
  { id: "images", label: "Images", icon: "◈" },
  { id: "networks", label: "Networks", icon: "⁂" },
  { id: "volumes", label: "Volumes", icon: "▤" },
  { id: "compose", label: "Compose", icon: "⧉" },
  { id: "kubernetes", label: "Kubernetes", icon: "☸" },
  { id: "activity", label: "Activity", icon: "◷" },
];

export default function App() {
  return (
    <ToastProvider>
      <Shell />
    </ToastProvider>
  );
}

function Shell() {
  const [view, setView] = useState<ViewId>("dashboard");
  // Collapsed by default; persisted, so anyone who expands it keeps that.
  const [collapsed, setCollapsed] = usePersisted("cleat.sidebar.collapsed", true);
  const [theme, setTheme] = useTheme();
  const [runtimes, setRuntimes] = useState<RuntimeInfo[]>([]);
  const [active, setActive] = useState<RuntimeKind | null>(null);
  const [booting, setBooting] = useState(true);
  const [switching, setSwitching] = useState(false);
  const toast = useToast();

  const refreshRuntimes = useCallback(async () => {
    try {
      const [list, current] = await Promise.all([api.listRuntimes(), api.currentRuntime()]);
      setRuntimes(list);
      setActive(current);
    } catch {
      /* the switcher just shows nothing selectable */
    }
  }, []);

  // Show a runtime only if it's on this machine. "Installed but not running" is
  // a fixable state worth surfacing with its remediation; "not installed" is
  // noise for someone who has never used that runtime. The active runtime is
  // always shown, so the UI can never hide what it is currently driving.
  const visibleRuntimes = useMemo(
    () => runtimes.filter((r) => r.installed || r.available || r.kind === active),
    [runtimes, active],
  );
  const hiddenCount = runtimes.length - visibleRuntimes.length;

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;

    // The backend auto-selects a runtime on startup and announces the result.
    api
      .onRuntimeReady(async (kind) => {
        if (cancelled) return;
        setActive(kind);
        setBooting(false);
        await refreshRuntimes();
      })
      .then((u) => {
        if (cancelled) u();
        else unlisten = u;
      });

    void (async () => {
      await refreshRuntimes();
      // If auto-selection finished before the listener attached, the event is
      // already gone; fall back to asking directly rather than hanging on the
      // boot spinner forever.
      const current = await api.currentRuntime().catch(() => null);
      if (!cancelled && current) {
        setActive(current);
        setBooting(false);
      }
    })();

    // Backstop: if neither path resolved, stop booting and show the
    // no-runtime screen rather than spinning indefinitely.
    const timeout = setTimeout(() => !cancelled && setBooting(false), 8000);

    return () => {
      cancelled = true;
      clearTimeout(timeout);
      unlisten?.();
    };
  }, [refreshRuntimes]);

  const switchTo = async (kind: RuntimeKind) => {
    if (kind === active) return;
    setSwitching(true);
    try {
      await api.selectRuntime(kind);
      setActive(kind);
      toast.success(`Switched to ${kind}`);
      await refreshRuntimes();
    } catch (e) {
      toast.failure(e);
    } finally {
      setSwitching(false);
    }
  };

  // Streams are per-runtime; drop them all when the window goes away.
  useEffect(() => {
    const onUnload = () => void api.stopAllStreams().catch(() => {});
    window.addEventListener("beforeunload", onUnload);
    return () => window.removeEventListener("beforeunload", onUnload);
  }, []);

  return (
    <div className="flex h-full flex-col">
      <div className="flex min-h-0 flex-1">
      <aside
        className={`no-select flex shrink-0 flex-col border-r border-edge bg-surface-1 transition-[width] duration-150 ${
          collapsed ? "w-[92px]" : "w-52"
        }`}
      >
        <div
          className={`flex items-center border-b border-edge py-3 ${
            collapsed ? "justify-between px-2" : "justify-between px-4"
          }`}
        >
          {/* The window itself wears the platform's title bar, which we cannot
              draw into, so the app's identity lives here — top-left, above the
              nav, present in both widths. */}
          <div className="flex min-w-0 items-center gap-2.5">
            <img
              src={logo}
              alt=""
              aria-hidden="true"
              width={collapsed ? 26 : 28}
              height={collapsed ? 26 : 28}
              className="shrink-0 rounded"
              // Collapsed, the mark is the only thing naming the app.
              title={collapsed ? "Cleat Cockpit" : undefined}
            />
            {!collapsed && (
              <div className="min-w-0">
                <div className="text-sm font-semibold text-ink">Cleat Cockpit</div>
                <div className="truncate text-xs text-ink-faint">
                  Docker · Podman · Kubernetes
                </div>
              </div>
            )}
          </div>
          <button
            onClick={() => setCollapsed(!collapsed)}
            title={collapsed ? "Expand sidebar" : "Collapse sidebar"}
            aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
            aria-expanded={!collapsed}
            className="rounded p-1 text-ink-faint transition-colors hover:bg-surface-2 hover:text-ink"
          >
            {collapsed ? "»" : "«"}
          </button>
        </div>

        <nav className="flex-1 space-y-0.5 p-2">
          {NAV.map((item) => (
            <button
              key={item.id}
              onClick={() => setView(item.id)}
              // The label is still rendered when collapsed, just small — an
              // icon-only rail of geometric glyphs is unreadable without one.
              // The tooltip carries it too, for the widths where it truncates.
              title={item.label}
              aria-current={view === item.id ? "page" : undefined}
              className={`flex w-full rounded-md transition-colors ${
                collapsed
                  ? "flex-col items-center gap-1 px-1 py-2"
                  : "items-center gap-2.5 px-3 py-2 text-left text-sm"
              } ${
                view === item.id
                  ? "bg-accent/15 font-medium text-accent"
                  : "text-ink-dim hover:bg-surface-2 hover:text-ink"
              }`}
            >
              <span
                className={collapsed ? "text-base leading-none opacity-80" : "w-4 text-center opacity-70"}
              >
                {item.icon}
              </span>
              <span className={collapsed ? "w-full truncate text-center text-[10px] leading-none" : ""}>
                {item.label}
              </span>
            </button>
          ))}
        </nav>

        <RuntimeSwitcher
          runtimes={visibleRuntimes}
          hiddenCount={hiddenCount}
          active={active}
          switching={switching}
          collapsed={collapsed}
          onSelect={switchTo}
          onRefresh={refreshRuntimes}
        />

      </aside>

      <main className="min-w-0 flex-1 overflow-hidden bg-surface-0">
        {booting ? (
          <div className="flex h-full flex-col items-center justify-center gap-3">
            <Spinner size={24} />
            <div className="text-xs text-ink-faint">Looking for a container runtime…</div>
          </div>
        ) : !active ? (
          <NoRuntime runtimes={runtimes} onRetry={refreshRuntimes} onSelect={switchTo} />
        ) : (
          // Keyed by runtime on purpose. Without this the views stay mounted
          // across a switch and keep rendering the *previous* runtime's data
          // until the next poll returns — so for a moment you see Docker's
          // images labelled as Podman's. Remounting resets them to a loading
          // state instead.
          <div key={active} className="h-full">
            {view === "dashboard" && <Dashboard onNavigate={(v) => setView(v as ViewId)} />}
            {view === "containers" && <Containers />}
            {view === "images" && <Images />}
            {view === "networks" && <Networks />}
            {view === "volumes" && <Volumes />}
            {view === "compose" && <Compose />}
            {view === "kubernetes" && <Kubernetes />}
            {view === "activity" && <Activity />}
          </div>
        )}
      </main>
      </div>

      <StatusBar
        runtimes={runtimes}
        active={active}
        theme={theme}
        onToggleTheme={() => setTheme(theme === "dark" ? "light" : "dark")}
      />
    </div>
  );
}

/**
 * Application status bar.
 *
 * The things worth knowing at a glance without navigating: which runtime is
 * driving, whether a cluster is reachable, and how much is running. Everything
 * here is already fetched or cheap to fetch — the summary is one call, and the
 * cluster probe only runs when a kubeconfig exists.
 *
 * The version comes from `tauri.conf.json` via a build-time define, so it
 * always matches the bundle actually produced — see `vite.config.ts`.
 */
function StatusBar({
  runtimes,
  active,
  theme,
  onToggleTheme,
}: {
  runtimes: RuntimeInfo[];
  active: RuntimeKind | null;
  theme: "dark" | "light";
  onToggleTheme: () => void;
}) {
  // Slow polls: this is peripheral vision, not a dashboard. A status bar that
  // refreshes as fast as the tables would double the app's idle load for
  // information nobody is staring at.
  const summary = usePolled<SystemSummary>(() => api.systemSummary(), 15_000, [active]);
  const [cluster, setCluster] = useState<{ context: string; info: ClusterInfo | null } | null>(
    null,
  );

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const contexts = await api.k8sContexts();
        if (cancelled || contexts.length === 0) return;
        const selected =
          (await api.k8sCurrentContext().catch(() => null)) ??
          contexts.find((c) => c.current)?.name ??
          contexts[0].name;
        if (cancelled) return;
        setCluster({ context: selected, info: null });

        const probes = await api.k8sProbeClusters();
        if (cancelled) return;
        setCluster({
          context: selected,
          info: probes.find((p) => p.context === selected) ?? null,
        });
      } catch {
        // No kubeconfig is the common case, not an error worth showing here.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const runtime = runtimes.find((r) => r.kind === active);
  const s = summary.data;

  return (
    <footer className="no-select flex shrink-0 items-center gap-3 border-t border-edge bg-surface-1 px-3 py-1 text-[11px] text-ink-faint">
      {/* Runtime */}
      <span className="flex items-center gap-1.5" title={runtime?.detail ?? undefined}>
        <Dot tone={active ? "ok" : "danger"} />
        <span className="text-ink-dim capitalize">{active ?? "no runtime"}</span>
        {runtime?.version && <span className="font-mono">{runtime.version}</span>}
      </span>

      {/* Cluster — absent entirely when there is no kubeconfig, rather than
          showing a permanently empty slot on machines with no cluster. */}
      {cluster && (
        <>
          <span className="text-edge">|</span>
          <span
            className="flex items-center gap-1.5"
            title={cluster.info?.detail ?? "Kubernetes context"}
          >
            <span aria-hidden="true">☸</span>
            <span className="max-w-48 truncate text-ink-dim">{cluster.context}</span>
            {cluster.info && (
              <span className={cluster.info.available ? "font-mono" : "text-danger"}>
                {cluster.info.available ? cluster.info.version : "unreachable"}
              </span>
            )}
          </span>
        </>
      )}

      {/* Counts */}
      {s && (
        <>
          <span className="text-edge">|</span>
          <span title="running / total containers">
            <span className="text-ok">{s.containersRunning}</span>
            <span className="text-ink-faint">
              {" "}
              of {s.containersRunning + s.containersPaused + s.containersStopped} running
            </span>
          </span>
          <span className="text-edge">|</span>
          <span title="images on this runtime">{s.images} images</span>
          <span className="text-edge">|</span>
          <span className="hidden sm:inline" title={`${s.os}/${s.arch}`}>
            {s.cpus} CPU · {formatBytes(s.memory)}
          </span>
        </>
      )}

      <div className="ml-auto flex items-center gap-3">
        {/* The heart uses the danger token rather than a literal red: it is the
            only red in the palette that is contrast-checked in both themes. */}
        <span className="hidden items-center gap-1 md:flex">
          Developed with <span className="text-danger">♥</span> by Algorisys Technologies
        </span>
        <ThemeToggle theme={theme} onToggle={onToggleTheme} />
        <UpdateStatus />
      </div>
    </footer>
  );
}

/**
 * The version, and whatever the updater currently has to say about it.
 *
 * Lives on the version itself because that is where someone looks when they
 * wonder whether they are current, and clicking it asks — an update check is a
 * question about this number. Nothing here is modal: the offer sits in the
 * status bar until it is taken, and declining it costs one click.
 */
function UpdateStatus() {
  const { state, check, install, dismiss } = useUpdates();
  const toast = useToast();
  const checking = state.status === "checking";

  // A manual check that finds nothing says so; the automatic one stays silent,
  // which is why this is here and not in the hook.
  const previous = useRef<UpdateState["status"]>("idle");
  useEffect(() => {
    if (state.status === "current" && previous.current === "checking") {
      toast.push("accent", `Cleat ${__APP_VERSION__} is the latest version`);
    }
    if (state.status === "failed" && previous.current !== "failed") {
      toast.failure(
        state.canSelfUpdate
          ? state.message
          : "This install cannot update itself — get the new version from Releases",
      );
    }
    previous.current = state.status;
  }, [state, toast]);

  return (
    <>
      {state.status === "downloading" && (
        <span className="text-accent">
          Downloading {state.version}
          {state.percent !== null ? ` · ${state.percent}%` : "…"}
        </span>
      )}

      {(state.status === "ready" || state.status === "installing") && (
        <span className="flex items-center gap-1.5">
          <button
            onClick={install}
            disabled={state.status === "installing"}
            title={`Install Cleat ${state.version} and restart`}
            className="rounded border border-accent/40 bg-accent/15 px-1.5 py-0.5 font-medium text-accent transition-colors hover:bg-accent/25 disabled:opacity-60"
          >
            {state.status === "installing"
              ? `Installing ${state.version}…`
              : `Update to ${state.version}`}
          </button>
          {state.status === "ready" && (
            <button
              onClick={dismiss}
              title="Not now — the update is kept and offered again next launch"
              className="rounded px-1 text-ink-faint transition-colors hover:text-ink"
            >
              ✕
            </button>
          )}
        </span>
      )}

      {state.status === "failed" && !state.canSelfUpdate && (
        <a
          href={RELEASES_URL}
          target="_blank"
          rel="noreferrer"
          className="text-accent underline-offset-2 hover:underline"
        >
          New version available
        </a>
      )}

      <button
        onClick={check}
        disabled={checking}
        title={checking ? "Checking for updates…" : `Cleat Cockpit ${__APP_VERSION__} — check for updates`}
        className="flex items-center gap-1.5 rounded px-1 font-mono transition-colors hover:text-ink disabled:opacity-60"
      >
        {checking && <Spinner size={10} />}
        v{__APP_VERSION__}
      </button>
    </>
  );
}

function RuntimeSwitcher({
  runtimes,
  hiddenCount,
  active,
  switching,
  collapsed,
  onSelect,
  onRefresh,
}: {
  runtimes: RuntimeInfo[];
  hiddenCount: number;
  active: RuntimeKind | null;
  switching: boolean;
  collapsed: boolean;
  onSelect: (k: RuntimeKind) => void;
  onRefresh: () => void;
}) {
  /** Everything the expanded rows show, folded into one tooltip. */
  const describe = (r: RuntimeInfo) =>
    [
      r.kind,
      r.version ? `v${r.version}` : null,
      r.available ? null : "unavailable",
      r.detail,
    ]
      .filter(Boolean)
      .join(" — ");

  // Collapsed, the version numbers and the "Runtime" heading do not fit, and
  // truncating a version to three characters would be worse than omitting it.
  // What has to survive is which runtime is active and whether the other one
  // can be switched to — a dot and a name carry both, and the tooltip carries
  // the rest.
  if (collapsed) {
    return (
      <div className="space-y-0.5 border-t border-edge p-1.5">
        {runtimes.map((r) => (
          <button
            key={r.kind}
            disabled={!r.available || switching}
            onClick={() => onSelect(r.kind)}
            title={describe(r)}
            aria-label={describe(r)}
            aria-current={active === r.kind ? "true" : undefined}
            className={`flex w-full flex-col items-center gap-0.5 rounded px-1 py-1.5 transition-colors disabled:cursor-not-allowed ${
              active === r.kind
                ? "bg-surface-3 text-ink"
                : r.available
                  ? "text-ink-dim hover:bg-surface-2"
                  : "text-ink-faint opacity-60"
            }`}
          >
            {switching && active !== r.kind && r.available ? (
              <Spinner size={10} />
            ) : (
              <Dot tone={r.available ? (active === r.kind ? "ok" : "idle") : "danger"} />
            )}
            <span className="w-full truncate text-center text-[10px] capitalize leading-none">
              {r.kind}
            </span>
          </button>
        ))}
        {runtimes.length === 0 && (
          <div className="py-1 text-center text-[10px] text-ink-faint">…</div>
        )}
      </div>
    );
  }

  return (
    <div className="border-t border-edge p-2">
      <div className="flex items-center justify-between px-1 pb-1">
        <span className="text-xs text-ink-faint">Runtime</span>
        <button className="text-xs text-ink-faint hover:text-ink" onClick={onRefresh}>
          ↻
        </button>
      </div>
      <div className="space-y-0.5">
        {runtimes.map((r) => (
          <button
            key={r.kind}
            disabled={!r.available || switching}
            onClick={() => onSelect(r.kind)}
            title={r.detail ?? (r.version ? `v${r.version}` : undefined)}
            className={`flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-xs transition-colors disabled:cursor-not-allowed ${
              active === r.kind
                ? "bg-surface-3 text-ink"
                : r.available
                  ? "text-ink-dim hover:bg-surface-2"
                  : "text-ink-faint opacity-60"
            }`}
          >
            <Dot tone={r.available ? (active === r.kind ? "ok" : "idle") : "danger"} />
            <span className="flex-1 capitalize">{r.kind}</span>
            {switching && active !== r.kind && r.available ? (
              <Spinner size={10} />
            ) : (
              r.version && <span className="font-mono text-[10px] opacity-70">{r.version}</span>
            )}
          </button>
        ))}
        {runtimes.length === 0 && (
          <div className="px-2 py-1 text-xs text-ink-faint">detecting…</div>
        )}
        {hiddenCount > 0 && (
          <div
            className="px-2 pt-1 text-[10px] text-ink-faint"
            title="Runtimes not installed on this machine are hidden"
          >
            {hiddenCount} not installed
          </div>
        )}
      </div>
    </div>
  );
}

function NoRuntime({
  runtimes,
  onRetry,
  onSelect,
}: {
  runtimes: RuntimeInfo[];
  onRetry: () => void;
  onSelect: (k: RuntimeKind) => void;
}) {
  return (
    <div className="flex h-full items-center justify-center p-8">
      <div className="w-full max-w-lg space-y-4">
        <div>
          <h2 className="text-base font-semibold text-ink">No container runtime available</h2>
          <p className="mt-1 text-sm text-ink-dim">
            Neither Docker nor Podman answered. Start one and try again.
          </p>
        </div>

        <div className="space-y-2">
          {runtimes.map((r) => (
            <div key={r.kind} className="rounded-lg border border-edge bg-surface-1 px-4 py-3">
              <div className="flex items-center gap-2">
                <span className="text-sm font-medium text-ink capitalize">{r.kind}</span>
                <Badge tone={r.available ? "ok" : "danger"}>
                  {r.available ? "available" : "unavailable"}
                </Badge>
                {r.available && (
                  <Button
                    size="sm"
                    variant="primary"
                    className="ml-auto"
                    onClick={() => onSelect(r.kind)}
                  >
                    Use
                  </Button>
                )}
              </div>
              {r.detail && (
                <div className="mt-1.5 font-mono text-xs break-words text-ink-faint">
                  {r.detail}
                </div>
              )}
            </div>
          ))}
        </div>

        <Button variant="subtle" onClick={onRetry}>
          Check again
        </Button>
      </div>
    </div>
  );
}
