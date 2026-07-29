import { useCallback, useEffect, useMemo, useState } from "react";
import * as api from "../api";
import { useBusyMap, useDebounced, usePersisted, usePolled } from "../hooks";
import type {
  ClusterInfo,
  K8sConfigEntry,
  K8sEvent,
  GeneratedManifest,
  K8sDeployment,
  K8sNamespace,
  K8sNode,
  K8sPod,
  K8sService,
  KubeContext,
  ManifestOutcome,
} from "../types";
import { errorMessage } from "../types";
import {
  Badge,
  Button,
  CodeBlock,
  ConfirmDialog,
  Dot,
  EmptyState,
  ErrorNote,
  Input,
  Modal,
  Panel,
  Select,
  Spinner,
  Table,
  Td,
  Th,
  useToast,
} from "../ui";
import { formatDuration } from "../util";

type Tab = "pods" | "deployments" | "services" | "config" | "events" | "nodes";

const TABS: { id: Tab; label: string }[] = [
  { id: "pods", label: "Pods" },
  { id: "deployments", label: "Deployments" },
  { id: "services", label: "Services" },
  { id: "config", label: "Config" },
  { id: "events", label: "Events" },
  { id: "nodes", label: "Nodes" },
];

/** Every namespace, as the namespace filter's "all" option. */
const ALL_NAMESPACES = "__all__";

/**
 * Colour a pod's status.
 *
 * The status string is whatever the API server's most informative field said,
 * so this has to classify by meaning rather than match a closed set —
 * `CrashLoopBackOff`, `ImagePullBackOff` and `ErrImagePull` are all failures
 * and all arrive as free-form reasons.
 */
function statusTone(status: string): "ok" | "warn" | "danger" | "idle" {
  if (status === "Running" || status === "Succeeded" || status === "Ready") return "ok";
  if (status === "Pending" || status === "Terminating" || status === "ContainerCreating") {
    return "warn";
  }
  if (
    status === "Failed" ||
    status === "NotReady" ||
    status.includes("BackOff") ||
    status.includes("Err") ||
    status.includes("Evicted") ||
    status.includes("Unknown")
  ) {
    return "danger";
  }
  return "idle";
}

export default function Kubernetes() {
  const [context, setContext] = usePersisted<string | null>("cleat.k8s.context", null);
  const [namespace, setNamespace] = usePersisted("cleat.k8s.namespace", ALL_NAMESPACES);
  const [tab, setTab] = usePersisted<Tab>("cleat.k8s.tab", "pods");
  const [query, setQuery] = useState("");
  const search = useDebounced(query, 200);
  const toast = useToast();

  const [contexts, setContexts] = useState<KubeContext[] | null>(null);
  const [clusters, setClusters] = useState<ClusterInfo[]>([]);
  const [contextError, setContextError] = useState<unknown>(null);
  const [applyOpen, setApplyOpen] = useState(false);

  // Reading the kubeconfig contacts nothing, so it is cheap and can happen up
  // front; probing every cluster is not, so it runs separately and the tables
  // do not wait on it.
  const loadContexts = useCallback(async () => {
    try {
      const list = await api.k8sContexts();
      setContexts(list);
      setContextError(null);
      if (!context) {
        const current = list.find((c) => c.current);
        if (current) {
          await api.k8sSelectContext(current.name);
          setContext(current.name);
        }
      } else {
        await api.k8sSelectContext(context);
      }
      void api.k8sProbeClusters().then(setClusters).catch(() => setClusters([]));
    } catch (e) {
      setContextError(e);
      setContexts([]);
    }
    // `context` is deliberately not a dependency: this establishes the initial
    // selection, and re-running it on every change would fight the user.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    void loadContexts();
  }, [loadContexts]);

  const switchContext = async (name: string) => {
    try {
      await api.k8sSelectContext(name);
      setContext(name);
      // Namespaces are per-cluster; keeping the old one selected would filter
      // against something that may not exist here.
      setNamespace(ALL_NAMESPACES);
      toast.success(`Switched to ${name}`);
    } catch (e) {
      toast.failure(e);
    }
  };

  const ns = namespace === ALL_NAMESPACES ? null : namespace;
  const namespaces = usePolled<K8sNamespace[]>(
    () => api.k8sListNamespaces(),
    30_000,
    [context],
  );

  const cluster = clusters.find((c) => c.context === context);
  const connected = !contextError && (contexts?.length ?? 0) > 0;

  if (contextError) {
    return (
      <div className="p-4">
        <ErrorNote error={contextError} onRetry={loadContexts} />
        <p className="mt-2 px-4 text-xs text-ink-faint">
          Cleat reads the same kubeconfig <code className="text-ink-dim">kubectl</code> does.
          If that works and this does not, the message above is the reason.
        </p>
      </div>
    );
  }

  if (contexts === null) {
    return (
      <div className="flex h-full justify-center py-16">
        <Spinner size={22} />
      </div>
    );
  }

  if (contexts.length === 0) {
    return (
      <EmptyState
        title="No Kubernetes contexts found"
        hint="Cleat reads ~/.kube/config. Point kubectl at a cluster and it will appear here."
      />
    );
  }

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      <div className="flex flex-wrap items-center gap-2">
        <Select
          value={context ?? ""}
          onChange={(e) => void switchContext(e.target.value)}
          className="max-w-64 text-xs"
          title="kubeconfig context"
        >
          {contexts.map((c) => (
            <option key={c.name} value={c.name}>
              {c.name}
              {c.current ? " (current)" : ""}
            </option>
          ))}
        </Select>

        {cluster && (
          <span
            className="flex items-center gap-1.5 text-xs"
            title={cluster.detail ?? undefined}
          >
            <Dot tone={cluster.available ? "ok" : "danger"} />
            <span className="text-ink-faint">
              {cluster.available ? cluster.version : "unreachable"}
            </span>
          </span>
        )}

        <Select
          value={namespace}
          onChange={(e) => setNamespace(e.target.value)}
          className="max-w-56 text-xs"
          title="Namespace"
        >
          <option value={ALL_NAMESPACES}>All namespaces</option>
          {(namespaces.data ?? []).map((n) => (
            <option key={n.name} value={n.name}>
              {n.name}
            </option>
          ))}
        </Select>

        <Input
          placeholder="Filter by name…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className="w-56"
        />

        <div className="ml-auto flex items-center gap-2">
          <Button size="sm" variant="primary" onClick={() => setApplyOpen(true)}>
            Apply YAML
          </Button>
        </div>
      </div>

      <div className="flex gap-1 border-b border-edge">
        {TABS.map((t) => (
          <button
            key={t.id}
            onClick={() => setTab(t.id)}
            className={`-mb-px border-b-2 px-3 py-1.5 text-sm transition-colors ${
              tab === t.id
                ? "border-accent font-medium text-accent"
                : "border-transparent text-ink-dim hover:text-ink"
            }`}
          >
            {t.label}
          </button>
        ))}
      </div>

      <Panel className="min-h-0 flex-1 overflow-auto">
        {!connected ? (
          <EmptyState title="No cluster selected" />
        ) : tab === "pods" ? (
          <PodsTable namespace={ns} search={search} context={context} />
        ) : tab === "deployments" ? (
          <DeploymentsTable namespace={ns} search={search} context={context} />
        ) : tab === "services" ? (
          <ServicesTable namespace={ns} search={search} context={context} />
        ) : tab === "config" ? (
          <ConfigTable namespace={ns} search={search} context={context} />
        ) : tab === "events" ? (
          <EventsTable namespace={ns} search={search} context={context} />
        ) : (
          <NodesTable search={search} context={context} />
        )}
      </Panel>

      {applyOpen && (
        <ApplyManifestModal
          namespace={ns}
          onClose={() => setApplyOpen(false)}
        />
      )}
    </div>
  );
}

// ==================================================================== pods

function PodsTable({
  namespace,
  search,
  context,
}: {
  namespace: string | null;
  search: string;
  context: string | null;
}) {
  const pods = usePolled<K8sPod[]>(() => api.k8sListPods(namespace), 5000, [namespace, context]);
  const { busy, run } = useBusyMap();
  const toast = useToast();
  const [logsFor, setLogsFor] = useState<K8sPod | null>(null);
  const [inspectFor, setInspectFor] = useState<K8sPod | null>(null);
  const [deleting, setDeleting] = useState<K8sPod | null>(null);

  const rows = useMemo(() => {
    const list = pods.data ?? [];
    const q = search.trim().toLowerCase();
    return q
      ? list.filter(
          (p) =>
            p.name.toLowerCase().includes(q) ||
            p.namespace.toLowerCase().includes(q) ||
            (p.node ?? "").toLowerCase().includes(q),
        )
      : list;
  }, [pods.data, search]);

  if (pods.error && pods.initial) return <ErrorNote error={pods.error} onRetry={pods.reload} />;
  if (pods.initial) {
    return (
      <div className="flex justify-center py-16">
        <Spinner size={22} />
      </div>
    );
  }
  if (rows.length === 0) {
    return <EmptyState title={search ? "No pods match that filter" : "No pods"} />;
  }

  return (
    <>
      <Table>
        <thead>
          <tr>
            <Th className="w-8" />
            <Th>Name</Th>
            <Th>Namespace</Th>
            <Th>Ready</Th>
            <Th>Status</Th>
            <Th>Restarts</Th>
            <Th>Node</Th>
            <Th>Age</Th>
            <Th className="text-right">Actions</Th>
          </tr>
        </thead>
        <tbody>
          {rows.map((p) => (
            <tr key={`${p.namespace}/${p.name}`} className="hover:bg-surface-2/50">
              <Td>
                <Dot tone={statusTone(p.status)} />
              </Td>
              <Td className="font-medium break-all text-ink">{p.name}</Td>
              <Td className="text-ink-dim">{p.namespace}</Td>
              <Td className="font-mono text-xs text-ink-dim">{p.ready}</Td>
              <Td>
                <Badge tone={statusTone(p.status)}>{p.status}</Badge>
              </Td>
              <Td className="text-ink-dim">
                {/* A restart count that is climbing is the single most useful
                    signal a pod list carries, so it is called out rather than
                    rendered as one more grey number. */}
                {p.restarts > 0 ? (
                  <span className="text-warn">{p.restarts}</span>
                ) : (
                  <span className="text-ink-faint">0</span>
                )}
              </Td>
              <Td className="max-w-40 truncate text-xs text-ink-faint" title={p.node ?? ""}>
                {p.node ?? "—"}
              </Td>
              <Td className="text-xs whitespace-nowrap text-ink-faint">
                {formatDuration(p.age)}
              </Td>
              <Td>
                <div className="flex justify-end gap-1">
                  <Button size="sm" variant="ghost" onClick={() => setLogsFor(p)}>
                    Logs
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setInspectFor(p)}>
                    Inspect
                  </Button>
                  <Button
                    size="sm"
                    variant="danger"
                    busy={busy[`${p.namespace}/${p.name}`]}
                    onClick={() => setDeleting(p)}
                  >
                    Delete
                  </Button>
                </div>
              </Td>
            </tr>
          ))}
        </tbody>
      </Table>

      {logsFor && <PodLogsModal pod={logsFor} onClose={() => setLogsFor(null)} />}
      {inspectFor && <PodInspectModal pod={inspectFor} onClose={() => setInspectFor(null)} />}

      <ConfirmDialog
        open={!!deleting}
        title={`Delete pod ${deleting?.name ?? ""}?`}
        confirmLabel="Delete"
        onCancel={() => setDeleting(null)}
        onConfirm={async () => {
          const pod = deleting!;
          setDeleting(null);
          await run(`${pod.namespace}/${pod.name}`, async () => {
            try {
              await api.k8sDeletePod(pod.namespace, pod.name);
              toast.success(`${pod.name} deleted`);
              pods.reload();
            } catch (e) {
              toast.failure(e);
            }
          });
        }}
        body={
          <p>
            {/* Whether this is destructive depends entirely on ownership, and
                the pod itself does not say — so state both cases rather than
                guessing one. */}
            If a Deployment or another controller owns this pod it will be
            recreated immediately, which is how a workload is restarted. If
            nothing owns it, it is gone.
          </p>
        }
      />
    </>
  );
}

function PodLogsModal({ pod, onClose }: { pod: K8sPod; onClose: () => void }) {
  const [container, setContainer] = useState<string>(pod.containers[0] ?? "");
  const [lines, setLines] = useState<string[]>([]);
  const [follow, setFollow] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const toast = useToast();

  useEffect(() => {
    let dispose: (() => void) | null = null;
    let cancelled = false;
    setLines([]);
    setError(null);
    setLoading(true);

    if (follow) {
      api
        .subscribePodLogs(
          pod.namespace,
          pod.name,
          container || null,
          (line) => setLines((prev) => [...prev.slice(-5000), line]),
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
            setError(errorMessage(e));
            setLoading(false);
          }
        });
    } else {
      api
        .k8sPodLogs(pod.namespace, pod.name, container || null, 1000)
        .then((text) => {
          if (cancelled) return;
          setLines(text.split("\n").filter(Boolean));
          setLoading(false);
        })
        .catch((e) => {
          if (!cancelled) {
            setError(errorMessage(e));
            setLoading(false);
          }
        });
    }

    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [pod.namespace, pod.name, container, follow]);

  const text = lines.join("\n");

  return (
    <Modal
      open
      onClose={onClose}
      title={`Logs — ${pod.name}`}
      subtitle={pod.namespace}
      width="max-w-5xl"
      footer={
        <>
          <span className="mr-auto text-xs text-ink-faint">{lines.length} lines</span>
          <Button
            variant="ghost"
            onClick={() => {
              void navigator.clipboard.writeText(text);
              toast.success("Logs copied");
            }}
          >
            Copy
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
          {pod.containers.length > 1 && (
            <Select
              value={container}
              onChange={(e) => setContainer(e.target.value)}
              className="text-xs"
            >
              {pod.containers.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </Select>
          )}
          {/* Deliberately no stdout/stderr filter: the API server merges the
              two and does not report which was which, so offering one would be
              a control that cannot work. */}
          {follow && !error && (
            <span className="ml-auto flex items-center gap-1 text-xs text-ok">
              <Dot tone="ok" /> live
            </span>
          )}
        </div>

        {error && (
          <div className="border-b border-danger/30 bg-danger/10 px-5 py-2 text-xs text-danger">
            {error}
          </div>
        )}

        {loading ? (
          <div className="flex justify-center py-16">
            <Spinner size={20} />
          </div>
        ) : lines.length === 0 ? (
          <EmptyState title="No log output" />
        ) : (
          <CodeBlock text={text} autoScroll={follow} className="h-[55vh]" />
        )}
      </div>
    </Modal>
  );
}

function PodInspectModal({ pod, onClose }: { pod: K8sPod; onClose: () => void }) {
  const [data, setData] = useState("");
  const [error, setError] = useState<unknown>(null);
  const toast = useToast();

  useEffect(() => {
    let cancelled = false;
    api
      .k8sInspectPod(pod.namespace, pod.name)
      .then((d) => !cancelled && setData(JSON.stringify(d, null, 2)))
      .catch((e) => !cancelled && setError(e));
    return () => {
      cancelled = true;
    };
  }, [pod.namespace, pod.name]);

  return (
    <Modal
      open
      onClose={onClose}
      title={`Inspect — ${pod.name}`}
      subtitle={pod.namespace}
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

// ============================================================= deployments

function DeploymentsTable({
  namespace,
  search,
  context,
}: {
  namespace: string | null;
  search: string;
  context: string | null;
}) {
  const items = usePolled<K8sDeployment[]>(
    () => api.k8sListDeployments(namespace),
    8000,
    [namespace, context],
  );
  const { busy, run } = useBusyMap();
  const toast = useToast();
  const [scaling, setScaling] = useState<K8sDeployment | null>(null);

  const rows = useMemo(() => {
    const list = items.data ?? [];
    const q = search.trim().toLowerCase();
    return q ? list.filter((d) => d.name.toLowerCase().includes(q)) : list;
  }, [items.data, search]);

  const restart = (d: K8sDeployment) =>
    run(`${d.namespace}/${d.name}:restart`, async () => {
      try {
        await api.k8sRestartDeployment(d.namespace, d.name);
        toast.success(`${d.name} rolling`);
        items.reload();
      } catch (e) {
        toast.failure(e);
      }
    });

  if (items.error && items.initial) return <ErrorNote error={items.error} onRetry={items.reload} />;
  if (items.initial) {
    return (
      <div className="flex justify-center py-16">
        <Spinner size={22} />
      </div>
    );
  }
  if (rows.length === 0) return <EmptyState title="No deployments" />;

  return (
    <>
    <Table>
      <thead>
        <tr>
          <Th>Name</Th>
          <Th>Namespace</Th>
          <Th>Ready</Th>
          <Th>Up to date</Th>
          <Th>Available</Th>
          <Th>Images</Th>
          <Th>Age</Th>
          <Th className="text-right">Actions</Th>
        </tr>
      </thead>
      <tbody>
        {rows.map((d) => {
          const [ready, desired] = d.ready.split("/").map(Number);
          const healthy = ready === desired && desired > 0;
          return (
            <tr key={`${d.namespace}/${d.name}`} className="hover:bg-surface-2/50">
              <Td className="font-medium text-ink">{d.name}</Td>
              <Td className="text-ink-dim">{d.namespace}</Td>
              <Td>
                <Badge tone={healthy ? "ok" : "warn"}>{d.ready}</Badge>
              </Td>
              <Td className="text-ink-dim">{d.upToDate}</Td>
              <Td className="text-ink-dim">{d.available}</Td>
              <Td className="max-w-64 truncate text-xs text-ink-faint" title={d.images.join("\n")}>
                {d.images.join(", ") || "—"}
              </Td>
              <Td className="text-xs whitespace-nowrap text-ink-faint">
                {formatDuration(d.age)}
              </Td>
              <Td>
                <div className="flex justify-end gap-1">
                  <Button size="sm" variant="ghost" onClick={() => setScaling(d)}>
                    Scale
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    busy={busy[`${d.namespace}/${d.name}:restart`]}
                    onClick={() => restart(d)}
                    title="Roll the pods, as `kubectl rollout restart` does"
                  >
                    Restart
                  </Button>
                </div>
              </Td>
            </tr>
          );
        })}
      </tbody>
    </Table>
    {scaling && (
      <ScaleDialog
        deployment={scaling}
        onClose={() => setScaling(null)}
        onScaled={() => {
          setScaling(null);
          items.reload();
        }}
      />
    )}
    </>
  );
}

/** Set a replica count, including zero. */
function ScaleDialog({
  deployment,
  onClose,
  onScaled,
}: {
  deployment: K8sDeployment;
  onClose: () => void;
  onScaled: () => void;
}) {
  const current = Number(deployment.ready.split("/")[1] ?? 0);
  const [replicas, setReplicas] = useState(String(current));
  const [busy, setBusy] = useState(false);
  const toast = useToast();
  const value = Number(replicas);
  const valid = Number.isInteger(value) && value >= 0;

  return (
    <Modal
      open
      onClose={onClose}
      title={`Scale ${deployment.name}`}
      subtitle={deployment.namespace}
      width="max-w-md"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            busy={busy}
            disabled={!valid || busy}
            onClick={async () => {
              setBusy(true);
              try {
                await api.k8sScaleDeployment(deployment.namespace, deployment.name, value);
                toast.success(`${deployment.name} scaled to ${value}`);
                onScaled();
              } catch (e) {
                toast.failure(e);
              } finally {
                setBusy(false);
              }
            }}
          >
            Scale
          </Button>
        </>
      }
    >
      <div className="space-y-2 px-5 py-4">
        <label className="mb-1 block text-xs text-ink-dim">Replicas</label>
        <Input
          autoFocus
          type="number"
          min={0}
          value={replicas}
          onChange={(e) => setReplicas(e.target.value)}
          className="w-full"
        />
        <p className="text-xs text-ink-faint">
          {/* Scaling to zero is a normal thing to want and easy to do by
              accident, so it is named rather than silently accepted. */}
          {value === 0
            ? "Zero replicas stops every pod but keeps the deployment, so it can be scaled back up."
            : `Currently ${current}.`}
        </p>
      </div>
    </Modal>
  );
}

// ================================================================ services

function ServicesTable({
  namespace,
  search,
  context,
}: {
  namespace: string | null;
  search: string;
  context: string | null;
}) {
  const items = usePolled<K8sService[]>(
    () => api.k8sListServices(namespace),
    8000,
    [namespace, context],
  );

  const rows = useMemo(() => {
    const list = items.data ?? [];
    const q = search.trim().toLowerCase();
    return q ? list.filter((s) => s.name.toLowerCase().includes(q)) : list;
  }, [items.data, search]);

  if (items.error && items.initial) return <ErrorNote error={items.error} onRetry={items.reload} />;
  if (items.initial) {
    return (
      <div className="flex justify-center py-16">
        <Spinner size={22} />
      </div>
    );
  }
  if (rows.length === 0) return <EmptyState title="No services" />;

  return (
    <Table>
      <thead>
        <tr>
          <Th>Name</Th>
          <Th>Namespace</Th>
          <Th>Type</Th>
          <Th>Cluster IP</Th>
          <Th>External IP</Th>
          <Th>Ports</Th>
          <Th>Age</Th>
        </tr>
      </thead>
      <tbody>
        {rows.map((s) => (
          <tr key={`${s.namespace}/${s.name}`} className="hover:bg-surface-2/50">
            <Td className="font-medium text-ink">{s.name}</Td>
            <Td className="text-ink-dim">{s.namespace}</Td>
            <Td>
              <Badge tone={s.type_ === "LoadBalancer" ? "accent" : "idle"}>{s.type_}</Badge>
            </Td>
            <Td className="font-mono text-xs text-ink-dim">{s.clusterIp ?? "—"}</Td>
            <Td className="font-mono text-xs text-ink-dim">
              {/* A LoadBalancer with no address yet is pending, not absent —
                  saying "—" would read as "this will never have one". */}
              {s.externalIp ?? (s.type_ === "LoadBalancer" ? (
                <span className="text-warn">pending</span>
              ) : (
                "—"
              ))}
            </Td>
            <Td className="font-mono text-xs text-ink-dim">{s.ports.join(", ") || "—"}</Td>
            <Td className="text-xs whitespace-nowrap text-ink-faint">{formatDuration(s.age)}</Td>
          </tr>
        ))}
      </tbody>
    </Table>
  );
}

// =================================================================== nodes

function NodesTable({ search, context }: { search: string; context: string | null }) {
  const items = usePolled<K8sNode[]>(() => api.k8sListNodes(), 15_000, [context]);

  const rows = useMemo(() => {
    const list = items.data ?? [];
    const q = search.trim().toLowerCase();
    return q ? list.filter((n) => n.name.toLowerCase().includes(q)) : list;
  }, [items.data, search]);

  if (items.error && items.initial) return <ErrorNote error={items.error} onRetry={items.reload} />;
  if (items.initial) {
    return (
      <div className="flex justify-center py-16">
        <Spinner size={22} />
      </div>
    );
  }
  if (rows.length === 0) return <EmptyState title="No nodes" />;

  return (
    <Table>
      <thead>
        <tr>
          <Th className="w-8" />
          <Th>Name</Th>
          <Th>Status</Th>
          <Th>Roles</Th>
          <Th>Version</Th>
          <Th>Internal IP</Th>
          <Th>Age</Th>
        </tr>
      </thead>
      <tbody>
        {rows.map((n) => (
          <tr key={n.name} className="hover:bg-surface-2/50">
            <Td>
              <Dot tone={statusTone(n.status)} />
            </Td>
            <Td className="font-medium text-ink">{n.name}</Td>
            <Td>
              <Badge tone={statusTone(n.status)}>{n.status}</Badge>
            </Td>
            <Td className="text-ink-dim">{n.roles.join(", ") || "—"}</Td>
            <Td className="font-mono text-xs text-ink-dim">{n.version}</Td>
            <Td className="font-mono text-xs text-ink-dim">{n.internalIp ?? "—"}</Td>
            <Td className="text-xs whitespace-nowrap text-ink-faint">{formatDuration(n.age)}</Td>
          </tr>
        ))}
      </tbody>
    </Table>
  );
}

// ======================================================== apply a manifest

/**
 * Paste YAML, see what it would do, then decide.
 *
 * The dry run is not a nicety. Applying a manifest is the one thing Cleat can
 * do to a cluster whose blast radius is unbounded, and pasted YAML is
 * frequently unread. The server-side dry run runs validation, admission
 * webhooks and defaulting and reports per-document outcomes without persisting
 * anything, so "what will this do" is answerable before it is answered the
 * hard way.
 */
export function ApplyManifestModal({
  namespace,
  initialYaml,
  title = "Apply YAML",
  onClose,
  onApplied,
}: {
  namespace: string | null;
  initialYaml?: string;
  title?: string;
  onClose: () => void;
  onApplied?: () => void;
}) {
  const [yaml, setYaml] = useState(initialYaml ?? "");
  const [outcomes, setOutcomes] = useState<ManifestOutcome[] | null>(null);
  const [checked, setChecked] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const toast = useToast();

  // Editing invalidates a previous dry run: the outcomes on screen would then
  // describe a document that no longer exists.
  useEffect(() => {
    setChecked(false);
    setOutcomes(null);
  }, [yaml]);

  const run = async (dryRun: boolean) => {
    if (!yaml.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const result = await api.k8sApplyManifest(yaml, namespace, dryRun);
      setOutcomes(result);
      setChecked(dryRun);
      if (!dryRun) {
        const failed = result.filter((o) => o.action === "failed").length;
        if (failed === 0) {
          toast.success(`Applied ${result.length} resource${result.length === 1 ? "" : "s"}`);
          onApplied?.();
        } else {
          toast.push("danger", `${failed} of ${result.length} failed`);
        }
      }
    } catch (e) {
      setError(e);
      setOutcomes(null);
    } finally {
      setBusy(false);
    }
  };

  const failures = outcomes?.filter((o) => o.action === "failed") ?? [];
  const applied = !!outcomes && !checked;

  return (
    <Modal
      open
      onClose={onClose}
      title={title}
      subtitle={namespace ? `default namespace: ${namespace}` : "namespace from each document"}
      width="max-w-4xl"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Close
          </Button>
          <Button
            variant="subtle"
            busy={busy && !checked}
            disabled={!yaml.trim() || busy}
            onClick={() => void run(true)}
          >
            Dry run
          </Button>
          <Button
            variant="primary"
            busy={busy && checked}
            // Applying is gated on a clean dry run. A failed one means the API
            // server already rejected these documents; applying anyway just
            // reproduces the rejection more expensively.
            disabled={!checked || failures.length > 0 || busy}
            title={
              !checked
                ? "Run a dry run first"
                : failures.length > 0
                  ? "The dry run reported failures — fix them first"
                  : undefined
            }
            onClick={() => void run(false)}
          >
            Apply
          </Button>
        </>
      }
    >
      <div className="space-y-3 px-5 py-4">
        <textarea
          value={yaml}
          onChange={(e) => setYaml(e.target.value)}
          spellCheck={false}
          placeholder={"apiVersion: apps/v1\nkind: Deployment\n…"}
          className="h-64 w-full resize-y rounded-md border border-edge bg-surface-0 p-3 font-mono text-xs text-ink outline-none focus:border-accent/60"
        />

        {error ? <ErrorNote error={error} /> : null}

        {outcomes && (
          <div className="space-y-1.5">
            <div className="text-xs text-ink-dim">
              {checked ? "Dry run — nothing was changed" : "Applied"}
            </div>
            {outcomes.map((o, i) => (
              <div
                key={`${o.kind}/${o.name}/${i}`}
                className={`flex flex-wrap items-center gap-2 rounded border px-3 py-1.5 text-xs ${
                  o.action === "failed"
                    ? "border-danger/30 bg-danger/10"
                    : "border-edge bg-surface-2/40"
                }`}
              >
                <Badge
                  tone={
                    o.action === "failed"
                      ? "danger"
                      : o.action === "created"
                        ? "ok"
                        : o.action === "configured"
                          ? "accent"
                          : "idle"
                  }
                >
                  {o.action}
                </Badge>
                <span className="text-ink">{o.kind}</span>
                <span className="font-medium text-ink">{o.name}</span>
                {o.namespace && <span className="text-ink-faint">in {o.namespace}</span>}
                {o.error && (
                  <span className="w-full font-mono break-words text-danger">{o.error}</span>
                )}
              </div>
            ))}
          </div>
        )}

        {applied && failures.length === 0 && (
          <div className="rounded border border-ok/30 bg-ok/10 px-3 py-2 text-xs text-ok">
            Applied successfully.
          </div>
        )}
      </div>
    </Modal>
  );
}

/**
 * Review a generated manifest before it goes near a cluster.
 *
 * Generation is lossy by nature — a bind mount to a path on this machine has no
 * cluster equivalent — so the warnings are the point of this dialog, not
 * decoration. It hands off to the same apply flow, dry run included.
 */
export function DeployToClusterModal({
  title,
  generate,
  onClose,
}: {
  title: string;
  generate: () => Promise<GeneratedManifest>;
  onClose: () => void;
}) {
  const [result, setResult] = useState<GeneratedManifest | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [applying, setApplying] = useState(false);

  useEffect(() => {
    let cancelled = false;
    generate()
      .then((r) => !cancelled && setResult(r))
      .catch((e) => !cancelled && setError(e));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (applying && result) {
    return (
      <ApplyManifestModal
        namespace={null}
        initialYaml={result.yaml}
        title={title}
        onClose={onClose}
      />
    );
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={title}
      subtitle="Review before applying"
      width="max-w-4xl"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!result} onClick={() => setApplying(true)}>
            Continue
          </Button>
        </>
      }
    >
      <div className="space-y-3 px-5 py-4">
        {error ? (
          <ErrorNote error={error} />
        ) : !result ? (
          <div className="flex justify-center py-14">
            <Spinner size={20} />
          </div>
        ) : (
          <>
            {result.warnings.length > 0 && (
              <div className="space-y-1.5">
                {result.warnings.map((w, i) => (
                  <div
                    key={`${w.kind}-${i}`}
                    className="rounded border border-warn/30 bg-warn/10 px-3 py-2 text-xs text-warn"
                  >
                    {w.message}
                  </div>
                ))}
              </div>
            )}
            <CodeBlock text={result.yaml} className="h-80 rounded-md" />
            <p className="text-xs text-ink-faint">
              Nothing has been sent to the cluster yet. Continue opens the apply
              dialog, where a dry run runs first.
            </p>
          </>
        )}
      </div>
    </Modal>
  );
}

// ================================================================== events

/**
 * Recent cluster events.
 *
 * The first place to look when something will not start, and the reason
 * diagnosing a Pending pod usually means dropping to `kubectl describe`. Sorted
 * newest-first by *last seen*, not creation: a warning that fired an hour ago
 * and again ten seconds ago is current, and ordering by creation buries it.
 */
function EventsTable({
  namespace,
  search,
  context,
}: {
  namespace: string | null;
  search: string;
  context: string | null;
}) {
  const items = usePolled<K8sEvent[]>(
    () => api.k8sListEvents(namespace),
    10_000,
    [namespace, context],
  );
  const [warningsOnly, setWarningsOnly] = usePersisted("cleat.k8s.warningsOnly", false);

  const rows = useMemo(() => {
    const list = items.data ?? [];
    const q = search.trim().toLowerCase();
    return list.filter((e) => {
      if (warningsOnly && e.type_ !== "Warning") return false;
      if (!q) return true;
      return (
        e.object.toLowerCase().includes(q) ||
        e.reason.toLowerCase().includes(q) ||
        e.message.toLowerCase().includes(q)
      );
    });
  }, [items.data, search, warningsOnly]);

  if (items.error && items.initial) return <ErrorNote error={items.error} onRetry={items.reload} />;
  if (items.initial) {
    return (
      <div className="flex justify-center py-16">
        <Spinner size={22} />
      </div>
    );
  }

  return (
    <>
      <div className="flex items-center gap-2 border-b border-edge px-3 py-1.5">
        <label className="flex items-center gap-1.5 text-xs text-ink-dim">
          <input
            type="checkbox"
            checked={warningsOnly}
            onChange={(e) => setWarningsOnly(e.target.checked)}
            className="accent-accent"
          />
          Warnings only
        </label>
        <span className="ml-auto text-xs text-ink-faint">{rows.length} events</span>
      </div>

      {rows.length === 0 ? (
        <EmptyState
          title={warningsOnly ? "No warnings" : "No events"}
          hint="Clusters expire events after about an hour, so an empty list can just mean nothing has happened recently."
        />
      ) : (
        <Table>
          <thead>
            <tr>
              <Th className="w-8" />
              <Th>Object</Th>
              <Th>Reason</Th>
              <Th>Message</Th>
              <Th>Count</Th>
              <Th>Last seen</Th>
            </tr>
          </thead>
          <tbody>
            {rows.map((e, i) => (
              <tr key={`${e.object}-${e.reason}-${i}`} className="hover:bg-surface-2/50">
                <Td>
                  <Dot tone={e.type_ === "Warning" ? "danger" : "idle"} />
                </Td>
                <Td className="font-medium break-all text-ink">{e.object}</Td>
                <Td>
                  <Badge tone={e.type_ === "Warning" ? "danger" : "idle"}>{e.reason}</Badge>
                </Td>
                <Td className="max-w-xl break-words text-xs text-ink-dim">{e.message}</Td>
                <Td className="text-ink-dim">
                  {/* A count above one means it is recurring, which is a
                      different problem from a one-off. */}
                  {e.count > 1 ? <span className="text-warn">×{e.count}</span> : "1"}
                </Td>
                <Td className="text-xs whitespace-nowrap text-ink-faint">
                  {formatDuration(e.age)}
                </Td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
    </>
  );
}

// ================================================================== config

/**
 * ConfigMaps and Secrets in one table.
 *
 * Key names only — Secret *values* are never fetched. Putting cluster
 * credentials into this process would give it something it has no reason to
 * hold, and would be one screenshot away from a bug report. Which keys exist is
 * what you actually need to know when a pod is failing to mount one.
 */
function ConfigTable({
  namespace,
  search,
  context,
}: {
  namespace: string | null;
  search: string;
  context: string | null;
}) {
  const items = usePolled<K8sConfigEntry[]>(
    () => api.k8sListConfig(namespace),
    15_000,
    [namespace, context],
  );

  const rows = useMemo(() => {
    const list = items.data ?? [];
    const q = search.trim().toLowerCase();
    return q
      ? list.filter(
          (c) =>
            c.name.toLowerCase().includes(q) ||
            c.keys.some((k) => k.toLowerCase().includes(q)),
        )
      : list;
  }, [items.data, search]);

  if (items.error && items.initial) return <ErrorNote error={items.error} onRetry={items.reload} />;
  if (items.initial) {
    return (
      <div className="flex justify-center py-16">
        <Spinner size={22} />
      </div>
    );
  }
  if (rows.length === 0) return <EmptyState title="No ConfigMaps or Secrets" />;

  return (
    <Table>
      <thead>
        <tr>
          <Th>Name</Th>
          <Th>Namespace</Th>
          <Th>Kind</Th>
          <Th>Type</Th>
          <Th>Keys</Th>
          <Th>Age</Th>
        </tr>
      </thead>
      <tbody>
        {rows.map((c) => (
          <tr key={`${c.kind}/${c.namespace}/${c.name}`} className="hover:bg-surface-2/50">
            <Td className="font-medium break-all text-ink">{c.name}</Td>
            <Td className="text-ink-dim">{c.namespace}</Td>
            <Td>
              <Badge tone={c.kind === "Secret" ? "warn" : "idle"}>{c.kind}</Badge>
            </Td>
            <Td className="text-xs text-ink-faint">{c.type_ ?? "—"}</Td>
            <Td className="max-w-xl text-xs text-ink-dim" title={c.keys.join("\n")}>
              {c.keys.length === 0 ? (
                <span className="text-ink-faint">none</span>
              ) : (
                <span className="font-mono break-all">{c.keys.join(", ")}</span>
              )}
            </Td>
            <Td className="text-xs whitespace-nowrap text-ink-faint">{formatDuration(c.age)}</Td>
          </tr>
        ))}
      </tbody>
    </Table>
  );
}
