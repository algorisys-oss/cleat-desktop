/**
 * What Cleat actually did.
 *
 * Cleat holds root-equivalent control of the container daemon. A document
 * asserting it makes only the calls it claims to is weaker than showing them,
 * so every operation recorded by the audit decorator lands here.
 *
 * The entries are Engine API requests, not `docker` commands — Cleat speaks the
 * API directly, and printing a CLI equivalent would be showing a translation
 * while implying it was a recording. Compose rows are the exception and carry
 * the literal argv, because compose is a real subprocess.
 */

import { useMemo, useState } from "react";
import * as api from "../api";
import { usePolled } from "../hooks";
import type { ActivityEntry, OpKind } from "../types";
import {
  Badge,
  Button,
  EmptyState,
  ErrorNote,
  Input,
  Panel,
  Spinner,
  Table,
  Td,
  Th,
  useToast,
} from "../ui";

/** Reads are dominated by the 4s list poll and the 1 Hz stats stream. */
const DEFAULT_KINDS: OpKind[] = ["write"];

function timeOf(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(
    d.getMilliseconds(),
    3,
  )}`;
}

export default function Activity() {
  const entries = usePolled<ActivityEntry[]>(() => api.activityLog(), 1500);
  const [query, setQuery] = useState("");
  const [kinds, setKinds] = useState<OpKind[]>(DEFAULT_KINDS);
  const [failuresOnly, setFailuresOnly] = useState(false);
  const toast = useToast();

  const all = entries.data ?? [];

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    return all.filter((e) => {
      if (!kinds.includes(e.kind)) return false;
      if (failuresOnly && !e.error) return false;
      if (!q) return true;
      return (
        e.op.toLowerCase().includes(q) ||
        e.detail.toLowerCase().includes(q) ||
        e.args.some(([k, v]) => k.toLowerCase().includes(q) || v.toLowerCase().includes(q))
      );
    });
  }, [all, query, kinds, failuresOnly]);

  const toggleKind = (k: OpKind) =>
    setKinds((prev) => (prev.includes(k) ? prev.filter((x) => x !== k) : [...prev, k]));

  const copyAll = async () => {
    const text = rows
      .map((e) => {
        const args = e.args.map(([k, v]) => `${k}=${v}`).join(" ");
        const failed = e.error ? `  ERROR: ${e.error}` : "";
        return `${timeOf(e.at)}  ${e.runtime}  ${e.detail}${args ? `  [${args}]` : ""}  ${e.durationMs}ms${failed}`;
      })
      .join("\n");
    try {
      await navigator.clipboard.writeText(text);
      toast.success(`Copied ${rows.length} entries`);
    } catch {
      toast.failure("Could not access the clipboard");
    }
  };

  if (entries.error && entries.initial) {
    return <ErrorNote error={entries.error} onRetry={entries.reload} />;
  }

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      <div className="flex flex-wrap items-center gap-2">
        <Input
          placeholder="Filter by operation, path, or argument…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className="w-80"
        />
        {(["write", "read"] as OpKind[]).map((k) => (
          <label key={k} className="flex items-center gap-1.5 text-xs text-ink-dim">
            <input
              type="checkbox"
              checked={kinds.includes(k)}
              onChange={() => toggleKind(k)}
              className="accent-accent"
            />
            {k === "write" ? "Changes" : "Reads"}
          </label>
        ))}
        <label className="flex items-center gap-1.5 text-xs text-ink-dim">
          <input
            type="checkbox"
            checked={failuresOnly}
            onChange={(e) => setFailuresOnly(e.target.checked)}
            className="accent-accent"
          />
          Failures only
        </label>

        <div className="ml-auto flex items-center gap-2 text-xs text-ink-faint">
          {entries.loading && !entries.initial && <Spinner size={12} />}
          <span>
            {rows.length} of {all.length}
          </span>
          <Button size="sm" variant="ghost" disabled={!rows.length} onClick={() => void copyAll()}>
            Copy
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={() =>
              void api
                .clearActivityLog()
                .then(() => entries.reload())
                .catch(toast.failure)
            }
          >
            Clear
          </Button>
        </div>
      </div>

      <Panel className="min-h-0 flex-1 overflow-auto">
        {entries.initial ? (
          <div className="flex justify-center py-16">
            <Spinner size={22} />
          </div>
        ) : rows.length === 0 ? (
          <EmptyState
            title={all.length ? "Nothing matches those filters" : "No activity yet"}
            hint={
              all.length
                ? "Reads are hidden by default — polling and stats would bury everything else."
                : "Every call Cleat makes to Docker or Podman is recorded here."
            }
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th className="w-24">Time</Th>
                <Th className="w-20">Runtime</Th>
                <Th>Operation</Th>
                <Th className="w-20 text-right">Took</Th>
              </tr>
            </thead>
            <tbody>
              {rows.map((e) => (
                <tr key={e.seq} className={e.error ? "bg-danger/5" : undefined}>
                  <Td className="font-mono text-[11px] whitespace-nowrap text-ink-faint">
                    {timeOf(e.at)}
                  </Td>
                  <Td className="text-xs capitalize text-ink-dim">{e.runtime}</Td>
                  <Td>
                    <div className="flex flex-wrap items-center gap-1.5">
                      <Badge tone={e.error ? "danger" : e.kind === "write" ? "warn" : "idle"}>
                        {e.op}
                      </Badge>
                      <code className="font-mono text-[11px] text-ink-dim">{e.detail}</code>
                    </div>
                    {e.args.length > 0 && (
                      <div className="mt-1 flex flex-wrap gap-1">
                        {e.args.map(([k, v]) => (
                          <span
                            key={k}
                            className="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-[10px] text-ink-faint"
                          >
                            {k}={v}
                          </span>
                        ))}
                      </div>
                    )}
                    {e.error && (
                      <div className="mt-1 font-mono text-[11px] text-danger">{e.error}</div>
                    )}
                  </Td>
                  <Td className="text-right font-mono text-[11px] whitespace-nowrap text-ink-faint">
                    {e.durationMs} ms
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
      </Panel>

      <p className="text-[11px] text-ink-faint">
        These are Engine API requests, which is what Cleat actually sends — not reconstructed{" "}
        <code className="font-mono">docker</code> commands. Compose rows show the real argv, because
        compose is the one operation that runs a subprocess. Environment values that look like
        secrets are masked.
      </p>
    </div>
  );
}
