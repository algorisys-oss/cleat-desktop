import { useCallback, useEffect, useMemo, useState } from "react";
import * as api from "./api";
import { usePersisted } from "./hooks";
import type { RuntimeInfo, RuntimeKind } from "./types";
import { Badge, Button, Dot, Spinner, ToastProvider, useToast } from "./ui";
import Containers from "./views/Containers";
import Dashboard from "./views/Dashboard";
import Images from "./views/Images";
import Networks from "./views/Networks";
import Volumes from "./views/Volumes";
import Activity from "./views/Activity";
import Compose from "./views/Compose";

type ViewId =
  | "dashboard"
  | "containers"
  | "images"
  | "networks"
  | "volumes"
  | "compose"
  | "activity";

const NAV: { id: ViewId; label: string; icon: string }[] = [
  { id: "dashboard", label: "Dashboard", icon: "▦" },
  { id: "containers", label: "Containers", icon: "▣" },
  { id: "images", label: "Images", icon: "◈" },
  { id: "networks", label: "Networks", icon: "⁂" },
  { id: "volumes", label: "Volumes", icon: "▤" },
  { id: "compose", label: "Compose", icon: "⧉" },
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
  // Persisted rather than per-session: a sidebar that springs back open every
  // launch is worse than one that never collapsed.
  const [collapsed, setCollapsed] = usePersisted("cleat.sidebar.collapsed", false);
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
    <div className="flex h-full">
      <aside
        className={`no-select flex shrink-0 flex-col border-r border-edge bg-surface-1 transition-[width] duration-150 ${
          collapsed ? "w-[74px]" : "w-52"
        }`}
      >
        <div
          className={`flex items-center border-b border-edge py-3 ${
            collapsed ? "justify-center px-2" : "justify-between px-4"
          }`}
        >
          {!collapsed && (
            <div className="min-w-0">
              <div className="text-sm font-semibold text-ink">Cleat</div>
              <div className="truncate text-xs text-ink-faint">Docker · Podman</div>
            </div>
          )}
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

        <StatusBar collapsed={collapsed} />
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
            {view === "activity" && <Activity />}
          </div>
        )}
      </main>
    </div>
  );
}

/**
 * Bottom strip of the sidebar. The version comes from `tauri.conf.json` via a
 * build-time define, so it always matches the bundle that was actually
 * produced — see `vite.config.ts`. `/shipit` bumps that file.
 */
function StatusBar({ collapsed }: { collapsed: boolean }) {
  return (
    <div
      className={`flex items-center border-t border-edge py-1.5 ${
        collapsed ? "justify-center px-1" : "justify-between px-3"
      }`}
    >
      {/* The name is the droppable half: it is on the window title and the
          header already. The version is the point of this strip. */}
      {!collapsed && <span className="text-[10px] text-ink-faint">Cleat</span>}
      <span className="font-mono text-[10px] text-ink-faint" title={`Cleat ${__APP_VERSION__}`}>
        v{__APP_VERSION__}
      </span>
    </div>
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
