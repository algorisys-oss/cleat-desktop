import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as api from "../api";
import { useBusyMap, useDebounced, usePolled, useSelection } from "../hooks";
import type { Container, LogLine, ServiceEntry, Stats } from "../types";
import {
  Badge,
  BulkBar,
  BulkResultDialog,
  Button,
  CodeBlock,
  ConfirmDialog,
  Dot,
  EmptyState,
  ErrorNote,
  Input,
  Modal,
  Panel,
  SelectBox,
  Select,
  Spinner,
  Table,
  Td,
  Th,
  useToast,
} from "../ui";
import {
  formatAge,
  formatBytes,
  formatPorts,
  healthTone,
  runBulk,
  shortId,
  stateTone,
  type BulkFailure,
} from "../util";
import ExecTerminal from "./ExecTerminal";
import RunImage from "./RunImage";

export default function Containers() {
  const [showAll, setShowAll] = useState(true);
  const containers = usePolled<Container[]>(() => api.listContainers(showAll), 4000, [showAll]);
  const [query, setQuery] = useState("");
  const search = useDebounced(query, 200);
  const { busy, run } = useBusyMap();
  const toast = useToast();

  const [logsFor, setLogsFor] = useState<Container | null>(null);
  const [execFor, setExecFor] = useState<Container | null>(null);
  const [creating, setCreating] = useState(false);
  const [statsFor, setStatsFor] = useState<Container | null>(null);
  const [inspectFor, setInspectFor] = useState<Container | null>(null);
  const [servicesFor, setServicesFor] = useState<Container | null>(null);
  const [removing, setRemoving] = useState<Container | null>(null);
  const [removeVolumes, setRemoveVolumes] = useState(false);
  const [bulkRemoving, setBulkRemoving] = useState(false);
  const [bulkVerb, setBulkVerb] = useState<string | null>(null);
  const [bulkResult, setBulkResult] = useState<{
    verb: string;
    done: number;
    failures: BulkFailure[];
  } | null>(null);

  const knownIds = useMemo(() => (containers.data ?? []).map((c) => c.id), [containers.data]);
  const selection = useSelection(knownIds);

  const rows = useMemo(() => {
    const list = containers.data ?? [];
    const q = search.trim().toLowerCase();
    const filtered = q
      ? list.filter(
          (c) =>
            c.name.toLowerCase().includes(q) ||
            c.image.toLowerCase().includes(q) ||
            c.id.toLowerCase().startsWith(q) ||
            (c.composeProject ?? "").toLowerCase().includes(q),
        )
      : list;
    // Running first, then by name, so the things you act on are at the top.
    return [...filtered].sort((a, b) => {
      const ra = a.state === "running" ? 0 : 1;
      const rb = b.state === "running" ? 0 : 1;
      return ra !== rb ? ra - rb : a.name.localeCompare(b.name);
    });
  }, [containers.data, search]);

  const act = async (c: Container, verb: string, fn: () => Promise<unknown>, past: string) => {
    await run(`${c.id}:${verb}`, async () => {
      try {
        await fn();
        toast.success(`${c.name} ${past}`);
        containers.reload();
      } catch (e) {
        toast.failure(e);
      }
    });
  };

  const visibleIds = useMemo(() => rows.map((c) => c.id), [rows]);
  const selectedRows = useMemo(
    () => (containers.data ?? []).filter((c) => selection.selected.has(c.id)),
    [containers.data, selection.selected],
  );
  // Bulk actions apply to the eligible subset rather than erroring on the rest:
  // starting a selection that is half running should start the other half, not
  // report "container already started" five times.
  const startable = selectedRows.filter((c) => c.state !== "running" && c.state !== "paused");
  const liveSelected = selectedRows.filter((c) => c.state === "running" || c.state === "paused");

  const bulk = async (
    verb: string,
    past: string,
    targets: Container[],
    fn: (c: Container) => Promise<unknown>,
  ) => {
    if (targets.length === 0) return;
    setBulkVerb(verb);
    const result = await runBulk(targets, (c) => c.name, fn);
    setBulkVerb(null);
    containers.reload();
    if (result.failures.length === 0) {
      toast.success(`${result.done} container${result.done === 1 ? "" : "s"} ${past}`);
      selection.clear();
    } else {
      // Selection is left alone so the failures stay visible and retryable.
      setBulkResult({ verb: past, done: result.done, failures: result.failures });
    }
  };

  if (containers.error && containers.initial) {
    return <ErrorNote error={containers.error} onRetry={containers.reload} />;
  }

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      <div className="flex items-center gap-2">
        <Input
          placeholder="Filter by name, image, id, or stack…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className="w-80"
        />
        <label className="flex items-center gap-1.5 text-xs text-ink-dim">
          <input
            type="checkbox"
            checked={showAll}
            onChange={(e) => setShowAll(e.target.checked)}
            className="accent-accent"
          />
          Show stopped
        </label>
        <div className="ml-auto flex items-center gap-2 text-xs text-ink-faint">
          {containers.loading && !containers.initial && <Spinner size={12} />}
          <span>{rows.length} shown</span>
          <Button size="sm" variant="ghost" onClick={containers.reload}>
            Refresh
          </Button>
          <Button size="sm" variant="primary" onClick={() => setCreating(true)}>
            New container
          </Button>
        </div>
      </div>

      {selection.size > 0 && (
        <BulkBar count={selection.size} noun="container" onClear={selection.clear}>
          <Button
            size="sm"
            variant="ghost"
            busy={bulkVerb === "start"}
            disabled={startable.length === 0 || !!bulkVerb}
            title={
              startable.length === 0
                ? "Every selected container is already running"
                : `Start ${startable.length} stopped container(s)`
            }
            onClick={() =>
              bulk("start", "started", startable, (c) => api.startContainer(c.id))
            }
          >
            Start {startable.length > 0 && startable.length}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            busy={bulkVerb === "stop"}
            disabled={liveSelected.length === 0 || !!bulkVerb}
            title={
              liveSelected.length === 0
                ? "No selected container is running"
                : `Stop ${liveSelected.length} running container(s)`
            }
            onClick={() => bulk("stop", "stopped", liveSelected, (c) => api.stopContainer(c.id))}
          >
            Stop {liveSelected.length > 0 && liveSelected.length}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            busy={bulkVerb === "restart"}
            disabled={liveSelected.length === 0 || !!bulkVerb}
            onClick={() =>
              bulk("restart", "restarted", liveSelected, (c) => api.restartContainer(c.id))
            }
          >
            Restart {liveSelected.length > 0 && liveSelected.length}
          </Button>
          <Button
            size="sm"
            variant="danger"
            busy={bulkVerb === "remove"}
            disabled={!!bulkVerb}
            onClick={() => {
              setRemoveVolumes(false);
              setBulkRemoving(true);
            }}
          >
            Remove {selection.size}
          </Button>
        </BulkBar>
      )}

      <Panel className="min-h-0 flex-1 overflow-auto">
        {containers.initial ? (
          <div className="flex justify-center py-16">
            <Spinner size={22} />
          </div>
        ) : rows.length === 0 ? (
          <EmptyState
            title={search ? "No containers match that filter" : "No containers"}
            hint={
              search
                ? undefined
                : showAll
                  ? "Nothing has been created on this runtime yet."
                  : "Nothing is running. Enable “Show stopped” to see exited containers."
            }
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th className="w-8">
                  <SelectBox
                    label="Select all shown containers"
                    checked={rows.every((c) => selection.selected.has(c.id))}
                    indeterminate={rows.some((c) => selection.selected.has(c.id))}
                    onToggle={() =>
                      selection.setMany(
                        visibleIds,
                        !rows.every((c) => selection.selected.has(c.id)),
                      )
                    }
                  />
                </Th>
                <Th className="w-8" />
                <Th>Name</Th>
                <Th>Image</Th>
                <Th>Status</Th>
                <Th>Ports</Th>
                <Th>Created</Th>
                <Th className="text-right">Actions</Th>
              </tr>
            </thead>
            <tbody>
              {rows.map((c) => {
                const running = c.state === "running";
                const paused = c.state === "paused";
                return (
                  <tr
                    key={c.id}
                    className={`group hover:bg-surface-2/50 ${
                      selection.selected.has(c.id) ? "bg-accent/8" : ""
                    }`}
                  >
                    <Td>
                      <SelectBox
                        label={`Select ${c.name}`}
                        checked={selection.selected.has(c.id)}
                        onToggle={(extend) => selection.toggle(c.id, visibleIds, extend)}
                      />
                    </Td>
                    <Td>
                      <Dot tone={stateTone(c.state)} />
                    </Td>
                    <Td>
                      <div className="font-medium text-ink">{c.name}</div>
                      <div className="flex items-center gap-1.5">
                        <span className="font-mono text-xs text-ink-faint">{shortId(c.id)}</span>
                        {c.composeProject && (
                          <span className="rounded bg-surface-3 px-1 text-[10px] text-ink-faint">
                            {c.composeProject}
                          </span>
                        )}
                      </div>
                    </Td>
                    <Td className="max-w-56 truncate text-ink-dim" title={c.image}>
                      {c.image}
                    </Td>
                    <Td>
                      <div className="flex flex-wrap items-center gap-1">
                        <Badge tone={stateTone(c.state)}>{c.status || c.state}</Badge>
                        {c.health !== "none" && (
                          <Badge tone={healthTone(c.health)}>{c.health}</Badge>
                        )}
                      </div>
                    </Td>
                    <Td className="font-mono text-xs text-ink-dim">{formatPorts(c.ports)}</Td>
                    <Td className="text-xs whitespace-nowrap text-ink-faint">
                      {formatAge(c.created)}
                    </Td>
                    <Td>
                      <div className="flex items-center justify-end gap-1">
                        {running || paused ? (
                          <Button
                            size="sm"
                            variant="ghost"
                            busy={busy[`${c.id}:stop`]}
                            onClick={() => act(c, "stop", () => api.stopContainer(c.id), "stopped")}
                          >
                            Stop
                          </Button>
                        ) : (
                          <Button
                            size="sm"
                            variant="ghost"
                            busy={busy[`${c.id}:start`]}
                            onClick={() =>
                              act(c, "start", () => api.startContainer(c.id), "started")
                            }
                          >
                            Start
                          </Button>
                        )}
                        <Button
                          size="sm"
                          variant="ghost"
                          busy={busy[`${c.id}:restart`]}
                          disabled={!running && !paused}
                          onClick={() =>
                            act(c, "restart", () => api.restartContainer(c.id), "restarted")
                          }
                        >
                          Restart
                        </Button>
                        {running && (
                          <Button
                            size="sm"
                            variant="ghost"
                            busy={busy[`${c.id}:pause`]}
                            onClick={() => act(c, "pause", () => api.pauseContainer(c.id), "paused")}
                          >
                            Pause
                          </Button>
                        )}
                        {paused && (
                          <Button
                            size="sm"
                            variant="ghost"
                            busy={busy[`${c.id}:unpause`]}
                            onClick={() =>
                              act(c, "unpause", () => api.unpauseContainer(c.id), "resumed")
                            }
                          >
                            Resume
                          </Button>
                        )}
                        <Button size="sm" variant="ghost" onClick={() => setLogsFor(c)}>
                          Logs
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          // Exec needs a live process namespace to join; the
                          // daemon rejects it otherwise.
                          disabled={!running}
                          onClick={() => setExecFor(c)}
                          title={
                            running
                              ? "Open an interactive shell in this container"
                              : "Container must be running"
                          }
                        >
                          Terminal
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          disabled={!running}
                          onClick={() => setStatsFor(c)}
                        >
                          Stats
                        </Button>
                        <Button size="sm" variant="ghost" onClick={() => setInspectFor(c)}>
                          Inspect
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          disabled={!running}
                          onClick={() => setServicesFor(c)}
                          title="Services managed by the container's init system"
                        >
                          Services
                        </Button>
                        <Button
                          size="sm"
                          variant="danger"
                          onClick={() => {
                            setRemoveVolumes(false);
                            setRemoving(c);
                          }}
                        >
                          Remove
                        </Button>
                      </div>
                    </Td>
                  </tr>
                );
              })}
            </tbody>
          </Table>
        )}
      </Panel>

      {logsFor && <LogsModal container={logsFor} onClose={() => setLogsFor(null)} />}
      {execFor && <ExecTerminal container={execFor} onClose={() => setExecFor(null)} />}
      {creating && (
        <RunImage
          image=""
          onClose={() => setCreating(false)}
          onCreated={() => containers.reload()}
        />
      )}
      {statsFor && <StatsModal container={statsFor} onClose={() => setStatsFor(null)} />}
      {inspectFor && <InspectModal container={inspectFor} onClose={() => setInspectFor(null)} />}
      {servicesFor && (
        <ServicesModal container={servicesFor} onClose={() => setServicesFor(null)} />
      )}

      <ConfirmDialog
        open={!!removing}
        title={`Remove ${removing?.name ?? ""}?`}
        confirmLabel="Remove"
        busy={!!removing && busy[`${removing.id}:remove`]}
        onCancel={() => setRemoving(null)}
        onConfirm={async () => {
          const c = removing!;
          setRemoving(null);
          await act(
            c,
            "remove",
            () => api.removeContainer(c.id, true, removeVolumes),
            "removed",
          );
        }}
        body={
          <p>
            This force-removes the container
            {removing?.state === "running" ? ", stopping it first" : ""}. This cannot be undone.
          </p>
        }
        extra={
          <label className="flex items-center gap-2 text-xs text-ink-dim">
            <input
              type="checkbox"
              checked={removeVolumes}
              onChange={(e) => setRemoveVolumes(e.target.checked)}
              className="accent-danger"
            />
            Also remove anonymous volumes
          </label>
        }
      />

      <ConfirmDialog
        open={bulkRemoving}
        title={`Remove ${selection.size} container${selection.size === 1 ? "" : "s"}?`}
        confirmLabel={`Remove ${selection.size}`}
        onCancel={() => setBulkRemoving(false)}
        onConfirm={() => {
          setBulkRemoving(false);
          void bulk("remove", "removed", selectedRows, (c) =>
            api.removeContainer(c.id, true, removeVolumes),
          );
        }}
        body={
          <>
            <p>
              This force-removes {selection.size} container
              {selection.size === 1 ? "" : "s"}
              {liveSelected.length > 0 &&
                `, stopping ${liveSelected.length} that ${
                  liveSelected.length === 1 ? "is" : "are"
                } still running`}
              . This cannot be undone.
            </p>
            <ul className="max-h-40 space-y-0.5 overflow-auto text-xs text-ink-faint">
              {selectedRows.map((c) => (
                <li key={c.id} className="truncate">
                  {c.name}
                </li>
              ))}
            </ul>
          </>
        }
        extra={
          <label className="flex items-center gap-2 text-xs text-ink-dim">
            <input
              type="checkbox"
              checked={removeVolumes}
              onChange={(e) => setRemoveVolumes(e.target.checked)}
              className="accent-danger"
            />
            Also remove anonymous volumes
          </label>
        }
      />

      <BulkResultDialog
        open={!!bulkResult}
        title="Some containers were not updated"
        done={bulkResult?.done ?? 0}
        verb={bulkResult?.verb ?? ""}
        failures={bulkResult?.failures ?? []}
        onClose={() => setBulkResult(null)}
      />
    </div>
  );
}

// ================================================================ logs modal

const MAX_LOG_LINES = 5000;

function LogsModal({ container, onClose }: { container: Container; onClose: () => void }) {
  const [lines, setLines] = useState<LogLine[]>([]);
  const [follow, setFollow] = useState(true);
  const [filter, setFilter] = useState("");
  const [streamFilter, setStreamFilter] = useState<"all" | "stdout" | "stderr">("all");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const search = useDebounced(filter, 150);
  const toast = useToast();

  useEffect(() => {
    let dispose: (() => void) | null = null;
    let cancelled = false;
    setLines([]);
    setError(null);
    setLoading(true);

    if (follow) {
      api
        .subscribeLogs(
          container.id,
          (line) =>
            setLines((prev) => {
              const next = [...prev, line];
              // Cap retained lines; a chatty container would otherwise grow the
              // array without bound for as long as the modal stays open.
              return next.length > MAX_LOG_LINES
                ? next.slice(next.length - MAX_LOG_LINES)
                : next;
            }),
          {
            tail: 500,
            onEnd: (err) => {
              if (cancelled) return;
              setError(err);
              setLoading(false);
            },
          },
        )
        .then((d) => {
          if (cancelled) d();
          else {
            dispose = d;
            setLoading(false);
          }
        })
        .catch((e) => {
          if (!cancelled) {
            setError(String(e));
            setLoading(false);
          }
        });
    } else {
      api
        .containerLogs(container.id, 1000, false)
        .then((text) => {
          if (cancelled) return;
          setLines(
            text
              .split("\n")
              .filter(Boolean)
              .map((message, seq) => ({
                containerId: container.id,
                stream: "stdout" as const,
                message,
                seq,
              })),
          );
          setLoading(false);
        })
        .catch((e) => {
          if (!cancelled) {
            setError(String(e));
            setLoading(false);
          }
        });
    }

    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [container.id, follow]);

  const visible = useMemo(() => {
    const q = search.trim().toLowerCase();
    return lines.filter(
      (l) =>
        (streamFilter === "all" || l.stream === streamFilter) &&
        (!q || l.message.toLowerCase().includes(q)),
    );
  }, [lines, search, streamFilter]);

  const text = useMemo(() => visible.map((l) => l.message).join("\n"), [visible]);

  return (
    <Modal
      open
      onClose={onClose}
      title={`Logs — ${container.name}`}
      subtitle={shortId(container.id)}
      width="max-w-5xl"
      footer={
        <>
          <span className="mr-auto text-xs text-ink-faint">
            {visible.length} of {lines.length} lines
            {lines.length >= MAX_LOG_LINES && ` (capped at ${MAX_LOG_LINES})`}
          </span>
          <Button
            variant="ghost"
            onClick={() => {
              void navigator.clipboard.writeText(text);
              toast.success("Logs copied");
            }}
          >
            Copy
          </Button>
          <Button variant="ghost" onClick={() => setLines([])}>
            Clear
          </Button>
          <Button variant="subtle" onClick={onClose}>
            Close
          </Button>
        </>
      }
    >
      <div className="flex flex-col">
        <div className="flex items-center gap-2 border-b border-edge px-5 py-2">
          <label className="flex items-center gap-1.5 text-xs text-ink-dim">
            <input
              type="checkbox"
              checked={follow}
              onChange={(e) => setFollow(e.target.checked)}
              className="accent-accent"
            />
            Follow
          </label>
          <Select
            value={streamFilter}
            onChange={(e) => setStreamFilter(e.target.value as typeof streamFilter)}
            className="text-xs"
          >
            <option value="all">All streams</option>
            <option value="stdout">stdout</option>
            <option value="stderr">stderr</option>
          </Select>
          <Input
            placeholder="Search lines…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            className="ml-auto w-64 text-xs"
          />
          {follow && !error && (
            <span className="flex items-center gap-1 text-xs text-ok">
              <Dot tone="ok" /> live
            </span>
          )}
        </div>

        {error && (
          <div className="border-b border-danger/30 bg-danger/10 px-5 py-2 text-xs text-danger">
            Stream ended: {error}
          </div>
        )}

        {loading ? (
          <div className="flex justify-center py-16">
            <Spinner size={20} />
          </div>
        ) : visible.length === 0 ? (
          <EmptyState
            title="No log output"
            hint={
              filter
                ? "No lines match the current search."
                : "This container hasn't written anything yet."
            }
          />
        ) : (
          <CodeBlock text={text} autoScroll={follow} className="h-[55vh]" />
        )}
      </div>
    </Modal>
  );
}

// =============================================================== stats modal

const HISTORY = 60;

function StatsModal({ container, onClose }: { container: Container; onClose: () => void }) {
  const [samples, setSamples] = useState<Stats[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let dispose: (() => void) | null = null;
    let cancelled = false;

    api
      .subscribeStats(
        container.id,
        (s) => setSamples((prev) => [...prev, s].slice(-HISTORY)),
        (err) => !cancelled && setError(err),
      )
      .then((d) => (cancelled ? d() : (dispose = d)))
      .catch((e) => !cancelled && setError(String(e)));

    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [container.id]);

  const latest = samples.at(-1);

  return (
    <Modal
      open
      onClose={onClose}
      title={`Stats — ${container.name}`}
      subtitle={shortId(container.id)}
      width="max-w-3xl"
      footer={
        <Button variant="subtle" onClick={onClose}>
          Close
        </Button>
      }
    >
      <div className="space-y-4 px-5 py-4">
        {error && (
          <div className="rounded border border-danger/30 bg-danger/10 px-3 py-2 text-xs text-danger">
            {error}
          </div>
        )}

        {!latest ? (
          <div className="flex justify-center py-14">
            <Spinner size={20} />
          </div>
        ) : (
          <>
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
              <Metric label="CPU" value={`${latest.cpuPercent.toFixed(1)}%`} />
              <Metric
                label="Memory"
                value={formatBytes(latest.memoryUsage)}
                sub={`${latest.memoryPercent.toFixed(1)}% of ${formatBytes(latest.memoryLimit)}`}
              />
              <Metric
                label="Network"
                value={`↓ ${formatBytes(latest.networkRx)}`}
                sub={`↑ ${formatBytes(latest.networkTx)}`}
              />
              <Metric
                label="Block I/O"
                value={`R ${formatBytes(latest.blockRead)}`}
                sub={`W ${formatBytes(latest.blockWrite)}`}
              />
            </div>

            <Sparkline
              title="CPU %"
              values={samples.map((s) => s.cpuPercent)}
              format={(v) => `${v.toFixed(1)}%`}
            />
            <Sparkline
              title="Memory"
              values={samples.map((s) => s.memoryUsage)}
              format={formatBytes}
            />

            <div className="text-xs text-ink-faint">
              {latest.pids} process{latest.pids === 1 ? "" : "es"} · {samples.length} sample
              {samples.length === 1 ? "" : "s"} · updates once per second
            </div>
          </>
        )}
      </div>
    </Modal>
  );
}

function Metric({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="rounded-lg border border-edge bg-surface-2/60 px-3 py-2">
      <div className="text-xs text-ink-faint">{label}</div>
      <div className="mt-0.5 font-mono text-base text-ink">{value}</div>
      {sub && <div className="font-mono text-xs text-ink-faint">{sub}</div>}
    </div>
  );
}

/** Minimal inline area chart; avoids pulling a charting library into the bundle. */
function Sparkline({
  title,
  values,
  format,
}: {
  title: string;
  values: number[];
  format: (v: number) => string;
}) {
  const width = 640;
  const height = 64;
  if (values.length < 2) {
    return (
      <div className="rounded-lg border border-edge bg-surface-2/40 px-3 py-2">
        <div className="text-xs text-ink-faint">{title}</div>
        <div className="py-4 text-center text-xs text-ink-faint">collecting…</div>
      </div>
    );
  }

  const max = Math.max(...values, 0.0001);
  const step = width / (values.length - 1);
  const points = values.map((v, i) => `${i * step},${height - (v / max) * (height - 6) - 3}`);
  const area = `0,${height} ${points.join(" ")} ${width},${height}`;

  return (
    <div className="rounded-lg border border-edge bg-surface-2/40 px-3 py-2">
      <div className="flex items-baseline justify-between">
        <span className="text-xs text-ink-faint">{title}</span>
        <span className="font-mono text-xs text-ink-dim">peak {format(max)}</span>
      </div>
      <svg
        viewBox={`0 0 ${width} ${height}`}
        preserveAspectRatio="none"
        className="mt-1 h-16 w-full"
      >
        <polygon points={area} fill="var(--color-accent)" opacity="0.18" />
        <polyline
          points={points.join(" ")}
          fill="none"
          stroke="var(--color-accent)"
          strokeWidth="1.5"
          vectorEffect="non-scaling-stroke"
        />
      </svg>
    </div>
  );
}

// ============================================================= inspect modal

function InspectModal({ container, onClose }: { container: Container; onClose: () => void }) {
  const [data, setData] = useState<string>("");
  const [error, setError] = useState<unknown>(null);
  const toast = useToast();

  useEffect(() => {
    let cancelled = false;
    api
      .inspectContainer(container.id)
      .then((d) => !cancelled && setData(JSON.stringify(d, null, 2)))
      .catch((e) => !cancelled && setError(e));
    return () => {
      cancelled = true;
    };
  }, [container.id]);

  return (
    <Modal
      open
      onClose={onClose}
      title={`Inspect — ${container.name}`}
      subtitle={shortId(container.id, 64)}
      width="max-w-4xl"
      footer={
        <>
          <Button
            variant="ghost"
            onClick={() => {
              void navigator.clipboard.writeText(data);
              toast.success("Copied to clipboard");
            }}
          >
            Copy JSON
          </Button>
          <Button variant="subtle" onClick={onClose}>
            Close
          </Button>
        </>
      }
    >
      {error ? (
        <ErrorNote error={error} />
      ) : !data ? (
        <div className="flex justify-center py-16">
          <Spinner size={20} />
        </div>
      ) : (
        <CodeBlock text={data} className="h-[60vh]" />
      )}
    </Modal>
  );
}

// ============================================================ services modal

function ServicesModal({ container, onClose }: { container: Container; onClose: () => void }) {
  const [services, setServices] = useState<ServiceEntry[] | null>(null);
  const [error, setError] = useState<unknown>(null);
  const { busy, run } = useBusyMap();
  const toast = useToast();
  const mounted = useRef(true);

  const load = useCallback(async () => {
    try {
      const list = await api.listContainerServices(container.id);
      if (mounted.current) {
        setServices(list);
        setError(null);
      }
    } catch (e) {
      if (mounted.current) setError(e);
    }
  }, [container.id]);

  useEffect(() => {
    mounted.current = true;
    void load();
    return () => {
      mounted.current = false;
    };
  }, [load]);

  const control = (service: string, action: string) =>
    run(`${service}:${action}`, async () => {
      try {
        await api.controlContainerService(container.id, service, action);
        toast.success(`${service} ${action}ed`);
        await load();
      } catch (e) {
        toast.failure(e);
      }
    });

  return (
    <Modal
      open
      onClose={onClose}
      title={`Services — ${container.name}`}
      subtitle={shortId(container.id)}
      width="max-w-2xl"
      footer={
        <>
          <Button variant="ghost" onClick={load}>
            Refresh
          </Button>
          <Button variant="subtle" onClick={onClose}>
            Close
          </Button>
        </>
      }
    >
      {error ? (
        <div className="px-5 py-4">
          <div className="rounded border border-warn/30 bg-warn/10 px-3 py-2 text-xs text-warn">
            Could not list services. This needs an init system (systemd) inside the container;
            most images don't have one.
          </div>
          <div className="mt-2 font-mono text-xs break-words text-ink-faint">
            {String((error as { message?: string })?.message ?? error)}
          </div>
        </div>
      ) : !services ? (
        <div className="flex justify-center py-14">
          <Spinner size={20} />
        </div>
      ) : services.length === 0 ? (
        <EmptyState title="No services reported" hint="systemctl returned no service units." />
      ) : (
        <Table>
          <thead>
            <tr>
              <Th>Unit</Th>
              <Th>State</Th>
              <Th className="text-right">Actions</Th>
            </tr>
          </thead>
          <tbody>
            {services.map((s) => (
              <tr key={s.name} className="hover:bg-surface-2/50">
                <Td className="font-mono text-xs">{s.name}</Td>
                <Td>
                  <Badge tone={s.state === "active" ? "ok" : s.state === "failed" ? "danger" : "idle"}>
                    {s.state}
                  </Badge>
                </Td>
                <Td>
                  <div className="flex justify-end gap-1">
                    <Button
                      size="sm"
                      variant="ghost"
                      busy={busy[`${s.name}:start`]}
                      onClick={() => control(s.name, "start")}
                    >
                      Start
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      busy={busy[`${s.name}:stop`]}
                      onClick={() => control(s.name, "stop")}
                    >
                      Stop
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      busy={busy[`${s.name}:restart`]}
                      onClick={() => control(s.name, "restart")}
                    >
                      Restart
                    </Button>
                  </div>
                </Td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
    </Modal>
  );
}
