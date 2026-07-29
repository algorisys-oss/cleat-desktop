import { useEffect, useMemo, useState } from "react";
import * as api from "../api";
import { useBusyMap, useDebounced, usePolled, useSelection } from "../hooks";
import type { Image, ImageTransferProgress, PullProgress, RuntimeInfo, RuntimeKind } from "../types";
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
  SelectBox,
  Spinner,
  Table,
  Td,
  Th,
  useToast,
} from "../ui";
import { errorMessage } from "../types";
import {
  formatAge,
  formatBytes,
  primaryTag,
  runBulk,
  shortId,
  type BulkFailure,
} from "../util";
import RunImage from "./RunImage";

export default function Images() {
  const images = usePolled<Image[]>(() => api.listImages(), 8000);
  const [query, setQuery] = useState("");
  const search = useDebounced(query, 200);
  const { busy, run } = useBusyMap();
  const toast = useToast();

  const [pullOpen, setPullOpen] = useState(false);
  const [copying, setCopying] = useState<{ image: Image; to: RuntimeKind } | null>(null);

  // Other runtimes we could copy an image into. Docker and Podman keep separate
  // image stores, so this is the only way to have one image on both.
  const [targets, setTargets] = useState<RuntimeInfo[]>([]);
  const [activeRuntime, setActiveRuntime] = useState<RuntimeKind | null>(null);
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const [list, current] = await Promise.all([api.listRuntimes(), api.currentRuntime()]);
        if (cancelled) return;
        setActiveRuntime(current);
        setTargets(list.filter((r) => r.available && r.kind !== current));
      } catch {
        /* copy action simply won't offer targets */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);
  const [removing, setRemoving] = useState<Image | null>(null);
  const [inspecting, setInspecting] = useState<Image | null>(null);
  const [pruning, setPruning] = useState(false);
  const [running, setRunning] = useState<string | null>(null);

  const [bulkRemoving, setBulkRemoving] = useState(false);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [bulkResult, setBulkResult] = useState<{ done: number; failures: BulkFailure[] } | null>(
    null,
  );

  const knownIds = useMemo(() => (images.data ?? []).map((i) => i.id), [images.data]);
  const selection = useSelection(knownIds);

  const rows = useMemo(() => {
    const list = images.data ?? [];
    const q = search.trim().toLowerCase();
    const filtered = q
      ? list.filter(
          (i) =>
            i.repoTags.some((t) => t.toLowerCase().includes(q)) ||
            shortId(i.id).toLowerCase().includes(q),
        )
      : list;
    return [...filtered].sort((a, b) => b.created - a.created);
  }, [images.data, search]);

  const totalSize = useMemo(() => rows.reduce((sum, i) => sum + i.size, 0), [rows]);

  const visibleIds = useMemo(() => rows.map((i) => i.id), [rows]);
  const selectedRows = useMemo(
    () => (images.data ?? []).filter((i) => selection.selected.has(i.id)),
    [images.data, selection.selected],
  );
  const selectedSize = selectedRows.reduce((sum, i) => sum + i.size, 0);
  const selectedInUse = selectedRows.filter((i) => i.containers > 0).length;

  const removeSelected = async () => {
    setBulkBusy(true);
    const result = await runBulk(
      selectedRows,
      (i) => primaryTag(i.repoTags),
      // Force for the same reason the single-image path does: a layer with more
      // than one tag, or one a container still references, is refused otherwise.
      (i) => api.removeImage(i.id, i.repoTags.length > 1 || i.containers > 0),
    );
    setBulkBusy(false);
    images.reload();
    if (result.failures.length === 0) {
      toast.success(`${result.done} image${result.done === 1 ? "" : "s"} removed`);
      selection.clear();
    } else {
      setBulkResult(result);
    }
  };

  if (images.error && images.initial) {
    return <ErrorNote error={images.error} onRetry={images.reload} />;
  }

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      <div className="flex items-center gap-2">
        <Input
          placeholder="Filter images…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className="w-72"
        />
        <Button variant="primary" size="sm" onClick={() => setPullOpen(true)}>
          Pull image
        </Button>
        <Button variant="subtle" size="sm" onClick={() => setPruning(true)}>
          Prune unused
        </Button>
        <div className="ml-auto flex items-center gap-2 text-xs text-ink-faint">
          {images.loading && !images.initial && <Spinner size={12} />}
          <span>
            {rows.length} images · {formatBytes(totalSize)}
          </span>
          <Button size="sm" variant="ghost" onClick={images.reload}>
            Refresh
          </Button>
        </div>
      </div>

      {selection.size > 0 && (
        <BulkBar count={selection.size} noun="image" onClear={selection.clear}>
          <Button
            size="sm"
            variant="danger"
            busy={bulkBusy}
            onClick={() => setBulkRemoving(true)}
          >
            Remove {selection.size}
          </Button>
          <span className="text-xs text-ink-faint">{formatBytes(selectedSize)}</span>
        </BulkBar>
      )}

      <Panel className="min-h-0 flex-1 overflow-auto">
        {images.initial ? (
          <div className="flex justify-center py-16">
            <Spinner size={22} />
          </div>
        ) : rows.length === 0 ? (
          <EmptyState
            title={search ? "No images match that filter" : "No images"}
            hint={search ? undefined : "Pull one to get started."}
            action={
              !search && (
                <Button variant="primary" size="sm" onClick={() => setPullOpen(true)}>
                  Pull image
                </Button>
              )
            }
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th className="w-8">
                  <SelectBox
                    label="Select all shown images"
                    checked={rows.every((i) => selection.selected.has(i.id))}
                    indeterminate={rows.some((i) => selection.selected.has(i.id))}
                    onToggle={() =>
                      selection.setMany(
                        visibleIds,
                        !rows.every((i) => selection.selected.has(i.id)),
                      )
                    }
                  />
                </Th>
                <Th>Repository / Tag</Th>
                <Th>Image ID</Th>
                <Th>Size</Th>
                <Th>In use</Th>
                <Th>Created</Th>
                <Th className="text-right">Actions</Th>
              </tr>
            </thead>
            <tbody>
              {rows.map((i) => (
                <tr
                  key={i.id}
                  className={`hover:bg-surface-2/50 ${
                    selection.selected.has(i.id) ? "bg-accent/8" : ""
                  }`}
                >
                  <Td>
                    <SelectBox
                      label={`Select ${primaryTag(i.repoTags)}`}
                      checked={selection.selected.has(i.id)}
                      onToggle={(extend) => selection.toggle(i.id, visibleIds, extend)}
                    />
                  </Td>
                  <Td>
                    <div className="font-medium text-ink">{primaryTag(i.repoTags)}</div>
                    {i.repoTags.length > 1 && (
                      <div className="text-xs text-ink-faint">
                        +{i.repoTags.length - 1} more tag{i.repoTags.length > 2 ? "s" : ""}
                      </div>
                    )}
                  </Td>
                  <Td className="font-mono text-xs text-ink-faint">{shortId(i.id)}</Td>
                  <Td className="text-ink-dim">{formatBytes(i.size)}</Td>
                  <Td>
                    {i.containers > 0 ? (
                      <Badge tone="accent">{i.containers} container{i.containers > 1 ? "s" : ""}</Badge>
                    ) : i.dangling ? (
                      <Badge tone="warn">dangling</Badge>
                    ) : (
                      <span className="text-xs text-ink-faint">—</span>
                    )}
                  </Td>
                  <Td className="text-xs whitespace-nowrap text-ink-faint">
                    {formatAge(i.created)}
                  </Td>
                  <Td>
                    <div className="flex justify-end gap-1">
                      {targets.map((t) => (
                        <Button
                          key={t.kind}
                          size="sm"
                          variant="ghost"
                          disabled={i.dangling}
                          title={
                            i.dangling
                              ? "Untagged images cannot be copied"
                              : `Copy this image into ${t.kind}`
                          }
                          onClick={() => setCopying({ image: i, to: t.kind })}
                        >
                          → {t.kind}
                        </Button>
                      ))}
                      <Button
                        size="sm"
                        variant="primary"
                        disabled={i.dangling}
                        title={
                          i.dangling
                            ? "Untagged images cannot be run by name"
                            : "Create and start a container from this image"
                        }
                        onClick={() => setRunning(primaryTag(i.repoTags))}
                      >
                        Run
                      </Button>
                      <Button size="sm" variant="ghost" onClick={() => setInspecting(i)}>
                        Inspect
                      </Button>
                      <Button size="sm" variant="danger" onClick={() => setRemoving(i)}>
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

      {pullOpen && (
        <PullModal
          onClose={() => {
            setPullOpen(false);
            images.reload();
          }}
        />
      )}

      {running && <RunImage image={running} onClose={() => setRunning(null)} />}

      {inspecting && <ImageInspectModal image={inspecting} onClose={() => setInspecting(null)} />}

      {copying && activeRuntime && (
        <CopyImageModal
          image={copying.image}
          from={activeRuntime}
          to={copying.to}
          onClose={() => setCopying(null)}
        />
      )}

      <ConfirmDialog
        open={!!removing}
        title={`Remove ${removing ? primaryTag(removing.repoTags) : ""}?`}
        confirmLabel="Remove"
        busy={!!removing && busy[`${removing.id}:remove`]}
        onCancel={() => setRemoving(null)}
        onConfirm={async () => {
          const img = removing!;
          setRemoving(null);
          await run(`${img.id}:remove`, async () => {
            try {
              // Force is needed whenever more than one tag points at the layer,
              // which is the common case for re-tagged images.
              await api.removeImage(img.id, img.repoTags.length > 1 || img.containers > 0);
              toast.success("Image removed");
              images.reload();
            } catch (e) {
              toast.failure(e);
            }
          });
        }}
        body={
          <p>
            {removing && removing.containers > 0
              ? `This image is used by ${removing.containers} container(s); removal will be forced.`
              : "This deletes the image layers from disk."}
          </p>
        }
      />

      <ConfirmDialog
        open={pruning}
        title="Prune unused images?"
        confirmLabel="Prune"
        onCancel={() => setPruning(false)}
        onConfirm={async () => {
          setPruning(false);
          try {
            const reclaimed = await api.pruneImages();
            toast.success(`Reclaimed ${formatBytes(reclaimed)}`);
            images.reload();
          } catch (e) {
            toast.failure(e);
          }
        }}
        body={<p>Removes every dangling image not referenced by a container.</p>}
      />

      <ConfirmDialog
        open={bulkRemoving}
        title={`Remove ${selection.size} image${selection.size === 1 ? "" : "s"}?`}
        confirmLabel={`Remove ${selection.size}`}
        onCancel={() => setBulkRemoving(false)}
        onConfirm={() => {
          setBulkRemoving(false);
          void removeSelected();
        }}
        body={
          <>
            <p>
              This deletes the layers from disk, reclaiming up to{" "}
              {formatBytes(selectedSize)}.
              {selectedInUse > 0 &&
                ` ${selectedInUse} of them ${
                  selectedInUse === 1 ? "is" : "are"
                } still used by a container; removal will be forced.`}
            </p>
            <ul className="max-h-40 space-y-0.5 overflow-auto text-xs text-ink-faint">
              {selectedRows.map((i) => (
                <li key={i.id} className="truncate">
                  {primaryTag(i.repoTags)}
                </li>
              ))}
            </ul>
          </>
        }
      />

      <BulkResultDialog
        open={!!bulkResult}
        title="Some images were not removed"
        done={bulkResult?.done ?? 0}
        verb="removed"
        failures={bulkResult?.failures ?? []}
        onClose={() => setBulkResult(null)}
      />
    </div>
  );
}

// ================================================================ pull modal

function PullModal({ onClose }: { onClose: () => void }) {
  const [name, setName] = useState("");
  const [pulling, setPulling] = useState(false);
  const [events, setEvents] = useState<PullProgress[]>([]);
  const [overall, setOverall] = useState<number | null>(null);
  const [finished, setFinished] = useState<{ ok: boolean; message: string } | null>(null);
  const [dispose, setDispose] = useState<(() => void) | null>(null);

  // Cancel a pull still in flight if the modal unmounts.
  useEffect(() => () => dispose?.(), [dispose]);

  const start = async () => {
    const image = name.trim();
    if (!image) return;
    setPulling(true);
    setEvents([]);
    setOverall(null);
    setFinished(null);

    try {
      const d = await api.subscribePull(image, (p) => {
        setEvents((prev) => [...prev.slice(-400), p]);
        if (p.overall !== null) setOverall(p.overall);
        if (p.done) {
          setPulling(false);
          setFinished(
            p.error
              ? { ok: false, message: p.error }
              : { ok: true, message: `Pulled ${image}` },
          );
          if (!p.error) setOverall(1);
        }
      });
      setDispose(() => d);
    } catch (e) {
      setPulling(false);
      setFinished({ ok: false, message: String((e as { message?: string })?.message ?? e) });
    }
  };

  // Latest status per layer, which is what a pull actually looks like.
  const layers = useMemo(() => {
    const map = new Map<string, PullProgress>();
    for (const e of events) {
      if (e.id) map.set(e.id, e);
    }
    return [...map.values()].slice(-14);
  }, [events]);

  const statusLine = events.at(-1)?.status ?? "";

  return (
    <Modal
      open
      onClose={onClose}
      title="Pull image"
      width="max-w-2xl"
      footer={
        <>
          {pulling ? (
            <Button
              variant="danger"
              onClick={() => {
                dispose?.();
                setDispose(null);
                setPulling(false);
                setFinished({ ok: false, message: "Pull cancelled" });
              }}
            >
              Cancel pull
            </Button>
          ) : (
            <Button variant="ghost" onClick={onClose}>
              Close
            </Button>
          )}
          <Button variant="primary" busy={pulling} disabled={!name.trim()} onClick={start}>
            {finished?.ok ? "Pull another" : "Pull"}
          </Button>
        </>
      }
    >
      <div className="space-y-4 px-5 py-4">
        <div>
          <label className="mb-1 block text-xs text-ink-dim">Image reference</label>
          <Input
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && !pulling && start()}
            placeholder="nginx:alpine, ghcr.io/owner/app:1.2.3"
            className="w-full"
            disabled={pulling}
          />
          <p className="mt-1 text-xs text-ink-faint">
            Without a tag, <code className="text-ink-dim">:latest</code> is used.
          </p>
        </div>

        {(pulling || overall !== null) && (
          <div>
            <div className="mb-1 flex justify-between text-xs text-ink-dim">
              <span className="truncate">{statusLine || "starting…"}</span>
              {overall !== null && <span>{Math.round(overall * 100)}%</span>}
            </div>
            <div className="h-2 overflow-hidden rounded-full bg-surface-3">
              <div
                className="h-full rounded-full bg-accent transition-[width] duration-200"
                style={{ width: `${Math.round((overall ?? 0) * 100)}%` }}
              />
            </div>
          </div>
        )}

        {layers.length > 0 && (
          <div className="max-h-52 space-y-1 overflow-auto rounded-md border border-edge bg-surface-0 p-2">
            {layers.map((l) => {
              const pct =
                l.total && l.total > 0 ? Math.round(((l.current ?? 0) / l.total) * 100) : null;
              return (
                <div key={l.id} className="flex items-center gap-2 text-xs">
                  <span className="w-16 shrink-0 font-mono text-ink-faint">{l.id}</span>
                  <span className="w-36 shrink-0 truncate text-ink-dim">{l.status}</span>
                  <div className="h-1 flex-1 overflow-hidden rounded-full bg-surface-2">
                    <div
                      className="h-full bg-accent/70"
                      style={{ width: `${pct ?? (l.status.includes("complete") ? 100 : 0)}%` }}
                    />
                  </div>
                  {pct !== null && <span className="w-9 text-right text-ink-faint">{pct}%</span>}
                </div>
              );
            })}
          </div>
        )}

        {finished && (
          <div
            className={`rounded border px-3 py-2 text-xs ${
              finished.ok
                ? "border-ok/30 bg-ok/10 text-ok"
                : "border-danger/30 bg-danger/10 text-danger"
            }`}
          >
            {finished.message}
          </div>
        )}
      </div>
    </Modal>
  );
}

// ============================================================= inspect modal

function ImageInspectModal({ image, onClose }: { image: Image; onClose: () => void }) {
  const [tab, setTab] = useState<"inspect" | "history">("inspect");
  const [data, setData] = useState<string>("");
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    let cancelled = false;
    setData("");
    setError(null);
    const load = tab === "inspect" ? api.inspectImage(image.id) : api.imageHistory(image.id);
    load
      .then((d) => !cancelled && setData(JSON.stringify(d, null, 2)))
      .catch((e) => !cancelled && setError(e));
    return () => {
      cancelled = true;
    };
  }, [image.id, tab]);

  return (
    <Modal
      open
      onClose={onClose}
      title={primaryTag(image.repoTags)}
      subtitle={shortId(image.id, 64)}
      width="max-w-4xl"
      footer={
        <Button variant="subtle" onClick={onClose}>
          Close
        </Button>
      }
    >
      <div className="flex gap-1 border-b border-edge px-5 py-2">
        {(["inspect", "history"] as const).map((t) => (
          <Button
            key={t}
            size="sm"
            variant={tab === t ? "subtle" : "ghost"}
            onClick={() => setTab(t)}
          >
            {t === "inspect" ? "Inspect" : "Layer history"}
          </Button>
        ))}
      </div>
      {error ? (
        <ErrorNote error={error} />
      ) : !data ? (
        <div className="flex justify-center py-16">
          <Spinner size={20} />
        </div>
      ) : (
        <CodeBlock text={data} className="h-[58vh]" />
      )}
    </Modal>
  );
}


// =========================================================== copy-to-runtime

/**
 * Copy an image into another runtime.
 *
 * Docker and Podman keep entirely separate image stores, so this physically
 * transfers the image: the source's export stream is piped straight into the
 * destination's import, without a temp file or a CLI.
 */
function CopyImageModal({
  image,
  from,
  to,
  onClose,
}: {
  image: Image;
  from: RuntimeKind;
  to: RuntimeKind;
  onClose: () => void;
}) {
  const reference = primaryTag(image.repoTags);
  const [progress, setProgress] = useState<ImageTransferProgress | null>(null);
  const [running, setRunning] = useState(false);
  const [finished, setFinished] = useState<{ ok: boolean; message: string } | null>(null);
  const [dispose, setDispose] = useState<(() => void) | null>(null);

  // Cancel an in-flight transfer if the modal unmounts.
  useEffect(() => () => dispose?.(), [dispose]);

  const start = async () => {
    setRunning(true);
    setProgress(null);
    setFinished(null);
    try {
      const d = await api.subscribeImageCopy(reference, to, (p) => {
        setProgress(p);
        if (p.done) {
          setRunning(false);
          setFinished(
            p.error
              ? { ok: false, message: p.error }
              : { ok: true, message: `Copied ${reference} to ${to}` },
          );
        }
      });
      setDispose(() => d);
    } catch (e) {
      setRunning(false);
      setFinished({ ok: false, message: errorMessage(e) });
    }
  };

  const pct = progress?.overall != null ? Math.round(progress.overall * 100) : null;

  return (
    <Modal
      open
      onClose={onClose}
      title={`Copy to ${to}`}
      subtitle={reference}
      width="max-w-lg"
      footer={
        <>
          {running ? (
            <Button
              variant="danger"
              onClick={() => {
                dispose?.();
                setDispose(null);
                setRunning(false);
                setFinished({ ok: false, message: "Transfer cancelled" });
              }}
            >
              Cancel
            </Button>
          ) : (
            <Button variant="ghost" onClick={onClose}>
              Close
            </Button>
          )}
          <Button variant="primary" busy={running} disabled={finished?.ok} onClick={start}>
            {finished?.ok ? "Done" : finished ? "Retry" : "Copy"}
          </Button>
        </>
      }
    >
      <div className="space-y-4 px-5 py-4">
        <div className="flex items-center justify-center gap-3 rounded-lg border border-edge bg-surface-2/50 px-4 py-3">
          <Badge tone="idle">{from}</Badge>
          <span className="text-ink-faint">→</span>
          <Badge tone="accent">{to}</Badge>
        </div>

        <p className="text-xs text-ink-dim">
          {from} and {to} keep separate image stores, so this transfers{" "}
          {formatBytes(image.size)} between them. The image stays in {from} as well.
        </p>

        {(running || progress) && (
          <div>
            <div className="mb-1 flex justify-between text-xs text-ink-dim">
              <span className="truncate">
                {progress?.phase === "importing"
                  ? (progress.status ?? "importing…")
                  : progress?.phase === "complete"
                    ? "complete"
                    : "transferring…"}
              </span>
              <span className="font-mono">
                {formatBytes(progress?.bytes ?? 0)}
                {pct !== null && ` · ${pct}%`}
              </span>
            </div>
            <div className="h-2 overflow-hidden rounded-full bg-surface-3">
              <div
                className="h-full rounded-full bg-accent transition-[width] duration-200"
                style={{ width: `${pct ?? 0}%` }}
              />
            </div>
          </div>
        )}

        {finished && (
          <div
            className={`rounded border px-3 py-2 text-xs ${
              finished.ok
                ? "border-ok/30 bg-ok/10 text-ok"
                : "border-danger/30 bg-danger/10 text-danger"
            }`}
          >
            {finished.message}
          </div>
        )}
      </div>
    </Modal>
  );
}
