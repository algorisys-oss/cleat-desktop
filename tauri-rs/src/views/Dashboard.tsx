import { usePolled } from "../hooks";
import type { Container, SystemSummary } from "../types";
import { Badge, EmptyState, ErrorNote, Panel, Spinner } from "../ui";
import * as api from "../api";
import { formatAge, formatBytes, stateTone } from "../util";

export default function Dashboard({ onNavigate }: { onNavigate: (view: string) => void }) {
  const summary = usePolled<SystemSummary>(() => api.systemSummary(), 10000);
  const containers = usePolled<Container[]>(() => api.listContainers(true), 6000);

  if (summary.error && summary.initial) {
    return <ErrorNote error={summary.error} onRetry={summary.reload} />;
  }

  if (summary.initial) {
    return (
      <div className="flex justify-center py-20">
        <Spinner size={24} />
      </div>
    );
  }

  const s = summary.data!;
  const recent = [...(containers.data ?? [])]
    .sort((a, b) => b.created - a.created)
    .slice(0, 8);

  return (
    <div className="h-full space-y-4 overflow-auto p-4">
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Stat
          label="Running"
          value={s.containersRunning}
          tone="ok"
          onClick={() => onNavigate("containers")}
        />
        <Stat
          label="Stopped"
          value={s.containersStopped}
          onClick={() => onNavigate("containers")}
        />
        <Stat label="Paused" value={s.containersPaused} tone="warn" />
        <Stat label="Images" value={s.images} onClick={() => onNavigate("images")} />
      </div>

      <div className="grid gap-4 lg:grid-cols-2">
        <Panel title="Runtime">
          <dl className="divide-y divide-edge/60">
            <Row label="Engine">
              <Badge tone="accent">{s.runtime}</Badge>
            </Row>
            <Row label="Version">{s.version}</Row>
            <Row label="API version">{s.apiVersion}</Row>
            <Row label="Platform">
              {s.os} / {s.arch}
            </Row>
            {s.kernel && <Row label="Kernel">{s.kernel}</Row>}
            {s.storageDriver && <Row label="Storage driver">{s.storageDriver}</Row>}
            <Row label="Host resources">
              {s.cpus} CPUs · {formatBytes(s.memory)} RAM
            </Row>
          </dl>
        </Panel>

        <Panel
          title="Recently created containers"
          actions={
            <button
              className="text-xs text-accent hover:underline"
              onClick={() => onNavigate("containers")}
            >
              View all
            </button>
          }
        >
          {recent.length === 0 ? (
            <EmptyState title="No containers yet" />
          ) : (
            <ul className="divide-y divide-edge/60">
              {recent.map((c) => (
                <li key={c.id} className="flex items-center gap-3 px-4 py-2">
                  <Badge tone={stateTone(c.state)}>{c.state}</Badge>
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm text-ink">{c.name}</div>
                    <div className="truncate text-xs text-ink-faint">{c.image}</div>
                  </div>
                  <span className="shrink-0 text-xs text-ink-faint">{formatAge(c.created)}</span>
                </li>
              ))}
            </ul>
          )}
        </Panel>
      </div>
    </div>
  );
}

function Stat({
  label,
  value,
  tone,
  onClick,
}: {
  label: string;
  value: number;
  tone?: "ok" | "warn";
  onClick?: () => void;
}) {
  const color = tone === "ok" ? "text-ok" : tone === "warn" ? "text-warn" : "text-ink";
  return (
    <button
      onClick={onClick}
      disabled={!onClick}
      className="rounded-lg border border-edge bg-surface-1 px-4 py-3 text-left transition-colors enabled:hover:border-accent/40 disabled:cursor-default"
    >
      <div className="text-xs text-ink-faint">{label}</div>
      <div className={`mt-1 font-mono text-2xl ${color}`}>{value}</div>
    </button>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 px-4 py-2 text-sm">
      <dt className="text-ink-faint">{label}</dt>
      <dd className="truncate font-mono text-xs text-ink-dim">{children}</dd>
    </div>
  );
}
