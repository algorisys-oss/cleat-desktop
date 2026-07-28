import { useEffect, useMemo, useState } from "react";
import * as api from "../api";
import { useBusyMap, useDebounced, usePolled } from "../hooks";
import type { Volume } from "../types";
import {
  Button,
  CodeBlock,
  ConfirmDialog,
  EmptyState,
  ErrorNote,
  Input,
  Modal,
  Panel,
  Spinner,
  Table,
  Td,
  Th,
  useToast,
} from "../ui";
import { formatBytes } from "../util";

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

  const rows = useMemo(() => {
    const list = volumes.data ?? [];
    const q = search.trim().toLowerCase();
    const filtered = q
      ? list.filter(
          (v) => v.name.toLowerCase().includes(q) || v.mountpoint.toLowerCase().includes(q),
        )
      : list;
    return [...filtered].sort((a, b) => a.name.localeCompare(b.name));
  }, [volumes.data, search]);

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

      <Panel className="min-h-0 flex-1 overflow-auto">
        {volumes.initial ? (
          <div className="flex justify-center py-16">
            <Spinner size={22} />
          </div>
        ) : rows.length === 0 ? (
          <EmptyState
            title={search ? "No volumes match that filter" : "No volumes"}
            hint={search ? undefined : "Named volumes you create will appear here."}
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>Name</Th>
                <Th>Driver</Th>
                <Th>Mount point</Th>
                <Th>Size</Th>
                <Th className="text-right">Actions</Th>
              </tr>
            </thead>
            <tbody>
              {rows.map((v) => (
                <tr key={v.name} className="hover:bg-surface-2/50">
                  <Td className="font-medium break-all text-ink">{v.name}</Td>
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
