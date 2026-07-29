import { useEffect, useMemo, useState } from "react";
import * as api from "../api";
import { useBusyMap, useDebounced, usePolled, useSelection } from "../hooks";
import type { Volume } from "../types";
import {
  Badge,
  BulkBar,
  BulkResultDialog,
  Button,
  CodeBlock,
  ConfirmDialog,
  EmptyState,
  ErrorNote,
  Input,
  Modal,
  Panel,
  Select,
  SelectBox,
  Spinner,
  Table,
  Td,
  Th,
  useToast,
} from "../ui";
import { formatBytes, isAnonymousVolume, runBulk, type BulkFailure } from "../util";

type Scope = "all" | "named" | "anonymous";

export default function Volumes() {
  const volumes = usePolled<Volume[]>(() => api.listVolumes(), 8000);
  const [query, setQuery] = useState("");
  const search = useDebounced(query, 200);
  const { busy, run } = useBusyMap();
  const toast = useToast();

  const [creating, setCreating] = useState(false);
  const [removing, setRemoving] = useState<Volume | null>(null);
  const [inspecting, setInspecting] = useState<Volume | null>(null);
  const [pruning, setPruning] = useState(false);
  const [bulkRemoving, setBulkRemoving] = useState(false);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [bulkResult, setBulkResult] = useState<{ done: number; failures: BulkFailure[] } | null>(
    null,
  );
  const [scope, setScope] = useState<Scope>("all");

  // Volumes are addressed by name, not by a separate id.
  const knownIds = useMemo(() => (volumes.data ?? []).map((v) => v.name), [volumes.data]);
  const selection = useSelection(knownIds);

  const rows = useMemo(() => {
    const list = volumes.data ?? [];
    const q = search.trim().toLowerCase();
    const filtered = list.filter((v) => {
      if (scope === "anonymous" && !isAnonymousVolume(v.name)) return false;
      if (scope === "named" && isAnonymousVolume(v.name)) return false;
      if (!q) return true;
      return v.name.toLowerCase().includes(q) || v.mountpoint.toLowerCase().includes(q);
    });
    return [...filtered].sort((a, b) => a.name.localeCompare(b.name));
  }, [volumes.data, search, scope]);

  const anonymousCount = useMemo(
    () => (volumes.data ?? []).filter((v) => isAnonymousVolume(v.name)).length,
    [volumes.data],
  );

  const visibleIds = useMemo(() => rows.map((v) => v.name), [rows]);
  const selectedRows = useMemo(
    () => (volumes.data ?? []).filter((v) => selection.selected.has(v.name)),
    [volumes.data, selection.selected],
  );
  const selectedSize = selectedRows.reduce(
    (sum, v) => sum + (v.size !== null && v.size > 0 ? v.size : 0),
    0,
  );

  const removeSelected = async () => {
    setBulkBusy(true);
    // Never forced: the daemon refusing to delete a volume a container still
    // mounts is the last thing standing between a stray click and lost data.
    const result = await runBulk(
      selectedRows,
      (v) => v.name,
      (v) => api.removeVolume(v.name, false),
    );
    setBulkBusy(false);
    volumes.reload();
    if (result.failures.length === 0) {
      toast.success(`${result.done} volume${result.done === 1 ? "" : "s"} removed`);
      selection.clear();
    } else {
      setBulkResult(result);
    }
  };

  if (volumes.error && volumes.initial) {
    return <ErrorNote error={volumes.error} onRetry={volumes.reload} />;
  }

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      <div className="flex items-center gap-2">
        <Input
          placeholder="Filter volumes…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className="w-72"
        />
        <Select
          value={scope}
          onChange={(e) => setScope(e.target.value as Scope)}
          className="text-xs"
          title="Anonymous volumes are the ones a container created for itself"
        >
          <option value="all">All volumes</option>
          <option value="named">Named only</option>
          <option value="anonymous">Anonymous only ({anonymousCount})</option>
        </Select>
        <Button variant="primary" size="sm" onClick={() => setCreating(true)}>
          Create volume
        </Button>
        <Button variant="subtle" size="sm" onClick={() => setPruning(true)}>
          Prune unused
        </Button>
        <div className="ml-auto flex items-center gap-2 text-xs text-ink-faint">
          {volumes.loading && !volumes.initial && <Spinner size={12} />}
          <span>{rows.length} volumes</span>
          <Button size="sm" variant="ghost" onClick={volumes.reload}>
            Refresh
          </Button>
        </div>
      </div>

      {selection.size > 0 && (
        <BulkBar count={selection.size} noun="volume" onClear={selection.clear}>
          <Button size="sm" variant="danger" busy={bulkBusy} onClick={() => setBulkRemoving(true)}>
            Remove {selection.size}
          </Button>
          {selectedSize > 0 && (
            <span className="text-xs text-ink-faint">{formatBytes(selectedSize)}</span>
          )}
        </BulkBar>
      )}

      <Panel className="min-h-0 flex-1 overflow-auto">
        {volumes.initial ? (
          <div className="flex justify-center py-16">
            <Spinner size={22} />
          </div>
        ) : rows.length === 0 ? (
          <EmptyState
            title={
              search || scope !== "all" ? "No volumes match that filter" : "No volumes"
            }
            hint={
              search || scope !== "all"
                ? undefined
                : "Named volumes you create will appear here."
            }
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th className="w-8">
                  <SelectBox
                    label="Select all shown volumes"
                    checked={rows.every((v) => selection.selected.has(v.name))}
                    indeterminate={rows.some((v) => selection.selected.has(v.name))}
                    onToggle={() =>
                      selection.setMany(
                        visibleIds,
                        !rows.every((v) => selection.selected.has(v.name)),
                      )
                    }
                  />
                </Th>
                <Th>Name</Th>
                <Th>Driver</Th>
                <Th>Mount point</Th>
                <Th>Size</Th>
                <Th className="text-right">Actions</Th>
              </tr>
            </thead>
            <tbody>
              {rows.map((v) => (
                <tr
                  key={v.name}
                  className={`hover:bg-surface-2/50 ${
                    selection.selected.has(v.name) ? "bg-accent/8" : ""
                  }`}
                >
                  <Td>
                    <SelectBox
                      label={`Select ${v.name}`}
                      checked={selection.selected.has(v.name)}
                      onToggle={(extend) => selection.toggle(v.name, visibleIds, extend)}
                    />
                  </Td>
                  <Td className="font-medium break-all text-ink" title={v.name}>
                    {isAnonymousVolume(v.name) ? (
                      <div className="flex items-center gap-1.5">
                        <span className="font-mono text-xs text-ink-dim">
                          {v.name.slice(0, 12)}…
                        </span>
                        <Badge tone="warn">anonymous</Badge>
                      </div>
                    ) : (
                      v.name
                    )}
                  </Td>
                  <Td className="text-ink-dim">{v.driver}</Td>
                  <Td
                    className="max-w-80 truncate font-mono text-xs text-ink-faint"
                    title={v.mountpoint}
                  >
                    {v.mountpoint}
                  </Td>
                  <Td className="text-ink-dim">
                    {/* The daemon reports -1 when it hasn't computed usage. */}
                    {v.size !== null && v.size >= 0 ? formatBytes(v.size) : "—"}
                  </Td>
                  <Td>
                    <div className="flex justify-end gap-1">
                      <Button size="sm" variant="ghost" onClick={() => setInspecting(v)}>
                        Inspect
                      </Button>
                      <Button size="sm" variant="danger" onClick={() => setRemoving(v)}>
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
        <CreateVolumeModal
          onClose={() => setCreating(false)}
          onCreated={() => {
            setCreating(false);
            volumes.reload();
          }}
        />
      )}

      {inspecting && <VolumeInspectModal volume={inspecting} onClose={() => setInspecting(null)} />}

      <ConfirmDialog
        open={!!removing}
        title={`Remove volume ${removing?.name ?? ""}?`}
        confirmLabel="Remove"
        busy={!!removing && busy[`${removing.name}:remove`]}
        onCancel={() => setRemoving(null)}
        onConfirm={async () => {
          const v = removing!;
          setRemoving(null);
          await run(`${v.name}:remove`, async () => {
            try {
              await api.removeVolume(v.name, false);
              toast.success(`Volume ${v.name} removed`);
              volumes.reload();
            } catch (e) {
              toast.failure(e);
            }
          });
        }}
        body={
          <p>
            <span className="text-danger">Data in this volume is deleted permanently.</span> If a
            container still uses it, the daemon will refuse.
          </p>
        }
      />

      <ConfirmDialog
        open={pruning}
        title="Prune unused volumes?"
        confirmLabel="Prune"
        onCancel={() => setPruning(false)}
        onConfirm={async () => {
          setPruning(false);
          try {
            const reclaimed = await api.pruneVolumes();
            toast.success(`Reclaimed ${formatBytes(reclaimed)}`);
            volumes.reload();
          } catch (e) {
            toast.failure(e);
          }
        }}
        body={
          <p>
            Deletes every volume not referenced by a container, and{" "}
            <span className="text-danger">all data inside them</span>.
          </p>
        }
      />

      <ConfirmDialog
        open={bulkRemoving}
        title={`Remove ${selection.size} volume${selection.size === 1 ? "" : "s"}?`}
        confirmLabel={`Remove ${selection.size}`}
        onCancel={() => setBulkRemoving(false)}
        onConfirm={() => {
          setBulkRemoving(false);
          void removeSelected();
        }}
        body={
          <>
            <p>
              <span className="text-danger">
                Data in {selection.size === 1 ? "this volume" : "these volumes"} is deleted
                permanently
              </span>
              {selectedSize > 0 && ` (${formatBytes(selectedSize)})`}. Any that a container still
              uses will be refused by the daemon and reported back.
            </p>
            <ul className="max-h-40 space-y-0.5 overflow-auto text-xs text-ink-faint">
              {selectedRows.map((v) => (
                <li key={v.name} className="truncate">
                  {v.name}
                </li>
              ))}
            </ul>
          </>
        }
      />

      <BulkResultDialog
        open={!!bulkResult}
        title="Some volumes were not removed"
        done={bulkResult?.done ?? 0}
        verb="removed"
        failures={bulkResult?.failures ?? []}
        onClose={() => setBulkResult(null)}
      />
    </div>
  );
}

function CreateVolumeModal({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: () => void;
}) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const toast = useToast();

  const submit = async () => {
    if (!name.trim()) return;
    setBusy(true);
    try {
      await api.createVolume(name.trim());
      toast.success(`Volume ${name.trim()} created`);
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
      title="Create volume"
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
      <div className="px-5 py-4">
        <label className="mb-1 block text-xs text-ink-dim">Name</label>
        <Input
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && submit()}
          placeholder="my-data"
          className="w-full"
        />
        <p className="mt-1 text-xs text-ink-faint">Uses the local driver.</p>
      </div>
    </Modal>
  );
}

function VolumeInspectModal({ volume, onClose }: { volume: Volume; onClose: () => void }) {
  const [data, setData] = useState("");
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .inspectVolume(volume.name)
      .then((d) => !cancelled && setData(JSON.stringify(d, null, 2)))
      .catch((e) => !cancelled && setError(e));
    return () => {
      cancelled = true;
    };
  }, [volume.name]);

  return (
    <Modal
      open
      onClose={onClose}
      title={`Inspect — ${volume.name}`}
      subtitle={volume.mountpoint}
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
        <CodeBlock text={data} className="h-[50vh]" />
      )}
    </Modal>
  );
}
