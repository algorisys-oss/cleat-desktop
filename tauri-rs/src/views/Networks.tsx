import { useEffect, useMemo, useState } from "react";
import * as api from "../api";
import { useBusyMap, useDebounced, usePolled } from "../hooks";
import type { Network } from "../types";
import {
  Badge,
  Button,
  CodeBlock,
  ConfirmDialog,
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
import { shortId } from "../util";

/** Networks Docker creates itself and refuses to delete. */
const BUILTIN = new Set(["bridge", "host", "none", "podman"]);

export default function Networks() {
  const networks = usePolled<Network[]>(() => api.listNetworks(), 8000);
  const [query, setQuery] = useState("");
  const search = useDebounced(query, 200);
  const { busy, run } = useBusyMap();
  const toast = useToast();

  const [creating, setCreating] = useState(false);
  const [removing, setRemoving] = useState<Network | null>(null);
  const [inspecting, setInspecting] = useState<Network | null>(null);

  const rows = useMemo(() => {
    const list = networks.data ?? [];
    const q = search.trim().toLowerCase();
    const filtered = q
      ? list.filter(
          (n) => n.name.toLowerCase().includes(q) || n.driver.toLowerCase().includes(q),
        )
      : list;
    return [...filtered].sort((a, b) => a.name.localeCompare(b.name));
  }, [networks.data, search]);

  if (networks.error && networks.initial) {
    return <ErrorNote error={networks.error} onRetry={networks.reload} />;
  }

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      <div className="flex items-center gap-2">
        <Input
          placeholder="Filter networks…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className="w-72"
        />
        <Button variant="primary" size="sm" onClick={() => setCreating(true)}>
          Create network
        </Button>
        <div className="ml-auto flex items-center gap-2 text-xs text-ink-faint">
          {networks.loading && !networks.initial && <Spinner size={12} />}
          <span>{rows.length} networks</span>
          <Button size="sm" variant="ghost" onClick={networks.reload}>
            Refresh
          </Button>
        </div>
      </div>

      <Panel className="min-h-0 flex-1 overflow-auto">
        {networks.initial ? (
          <div className="flex justify-center py-16">
            <Spinner size={22} />
          </div>
        ) : rows.length === 0 ? (
          <EmptyState title="No networks match that filter" />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>Name</Th>
                <Th>Driver</Th>
                <Th>Scope</Th>
                <Th>Subnet</Th>
                <Th>Containers</Th>
                <Th className="text-right">Actions</Th>
              </tr>
            </thead>
            <tbody>
              {rows.map((n) => (
                <tr key={n.id} className="hover:bg-surface-2/50">
                  <Td>
                    <div className="flex items-center gap-1.5">
                      <span className="font-medium text-ink">{n.name}</span>
                      {BUILTIN.has(n.name) && <Badge tone="idle">built-in</Badge>}
                      {n.internal && <Badge tone="warn">internal</Badge>}
                    </div>
                    <div className="font-mono text-xs text-ink-faint">{shortId(n.id)}</div>
                  </Td>
                  <Td className="text-ink-dim">{n.driver}</Td>
                  <Td className="text-ink-dim">{n.scope}</Td>
                  <Td className="font-mono text-xs text-ink-dim">
                    {n.subnets.length ? n.subnets.join(", ") : "—"}
                  </Td>
                  <Td>
                    {n.containers.length === 0 ? (
                      <span className="text-xs text-ink-faint">—</span>
                    ) : (
                      <span
                        className="text-xs text-ink-dim"
                        title={n.containers.join("\n")}
                      >
                        {n.containers.length} attached
                      </span>
                    )}
                  </Td>
                  <Td>
                    <div className="flex justify-end gap-1">
                      <Button size="sm" variant="ghost" onClick={() => setInspecting(n)}>
                        Inspect
                      </Button>
                      <Button
                        size="sm"
                        variant="danger"
                        disabled={BUILTIN.has(n.name)}
                        title={
                          BUILTIN.has(n.name)
                            ? "Built-in networks cannot be removed"
                            : undefined
                        }
                        onClick={() => setRemoving(n)}
                      >
                        Remove
                      </Button>
                    </div>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
      </Panel>

      {creating && (
        <CreateNetworkModal
          onClose={() => setCreating(false)}
          onCreated={() => {
            setCreating(false);
            networks.reload();
          }}
        />
      )}

      {inspecting && (
        <NetworkInspectModal network={inspecting} onClose={() => setInspecting(null)} />
      )}

      <ConfirmDialog
        open={!!removing}
        title={`Remove network ${removing?.name ?? ""}?`}
        confirmLabel="Remove"
        busy={!!removing && busy[`${removing.id}:remove`]}
        onCancel={() => setRemoving(null)}
        onConfirm={async () => {
          const n = removing!;
          setRemoving(null);
          await run(`${n.id}:remove`, async () => {
            try {
              await api.removeNetwork(n.id);
              toast.success(`Network ${n.name} removed`);
              networks.reload();
            } catch (e) {
              toast.failure(e);
            }
          });
        }}
        body={
          removing && removing.containers.length > 0 ? (
            <p>
              <span className="text-warn">
                {removing.containers.length} container(s) are still attached
              </span>{" "}
              — the daemon will refuse to remove this network until they are disconnected.
            </p>
          ) : (
            <p>This deletes the network definition.</p>
          )
        }
      />
    </div>
  );
}

function CreateNetworkModal({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: () => void;
}) {
  const [name, setName] = useState("");
  const [driver, setDriver] = useState("bridge");
  const [internal, setInternal] = useState(false);
  const [busy, setBusy] = useState(false);
  const toast = useToast();

  const submit = async () => {
    if (!name.trim()) return;
    setBusy(true);
    try {
      await api.createNetwork(name.trim(), driver, internal);
      toast.success(`Network ${name.trim()} created`);
      onCreated();
    } catch (e) {
      toast.failure(e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      onClose={onClose}
      title="Create network"
      width="max-w-md"
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" busy={busy} disabled={!name.trim()} onClick={submit}>
            Create
          </Button>
        </>
      }
    >
      <div className="space-y-3 px-5 py-4">
        <div>
          <label className="mb-1 block text-xs text-ink-dim">Name</label>
          <Input
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && submit()}
            placeholder="my-network"
            className="w-full"
          />
        </div>
        <div>
          <label className="mb-1 block text-xs text-ink-dim">Driver</label>
          <Select value={driver} onChange={(e) => setDriver(e.target.value)} className="w-full">
            <option value="bridge">bridge</option>
            <option value="overlay">overlay (swarm)</option>
            <option value="macvlan">macvlan</option>
            <option value="ipvlan">ipvlan</option>
          </Select>
        </div>
        <label className="flex items-center gap-2 text-xs text-ink-dim">
          <input
            type="checkbox"
            checked={internal}
            onChange={(e) => setInternal(e.target.checked)}
            className="accent-accent"
          />
          Internal (no external routing)
        </label>
      </div>
    </Modal>
  );
}

function NetworkInspectModal({
  network,
  onClose,
}: {
  network: Network;
  onClose: () => void;
}) {
  const [data, setData] = useState("");
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .inspectNetwork(network.id)
      .then((d) => !cancelled && setData(JSON.stringify(d, null, 2)))
      .catch((e) => !cancelled && setError(e));
    return () => {
      cancelled = true;
    };
  }, [network.id]);

  return (
    <Modal
      open
      onClose={onClose}
      title={`Inspect — ${network.name}`}
      subtitle={shortId(network.id, 64)}
      width="max-w-3xl"
      footer={
        <Button variant="subtle" onClick={onClose}>
          Close
        </Button>
      }
    >
      {error ? (
        <ErrorNote error={error} />
      ) : !data ? (
        <div className="flex justify-center py-16">
          <Spinner size={20} />
        </div>
      ) : (
        <CodeBlock text={data} className="h-[55vh]" />
      )}
    </Modal>
  );
}
