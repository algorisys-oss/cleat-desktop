/**
 * Create a container from an image.
 *
 * The backend `create_container` has existed and been tested since before there
 * was any UI for it; this is the form that finally reaches it. Two things about
 * the contract shape this component:
 *
 * 1. `create_container` **creates but does not start**. "Run" therefore issues
 *    create + start, and reports which of the two failed — a created-but-dead
 *    container is a confusing outcome to leave unexplained.
 * 2. `command` is exact argv, never a shell string. Naively splitting on spaces
 *    would silently turn `sh -c "while true; do :; done"` into seven arguments.
 *    Rather than ban quotes or split blindly, the field is tokenised with quote
 *    awareness and the result shown back as chips, so the tokenisation is never
 *    the silent part.
 */

import { useEffect, useMemo, useState } from "react";
import * as api from "../api";
import type { CreateContainerRequest, Network } from "../types";
import { errorMessage } from "../types";
import { Button, Input, Modal, Select, useToast } from "../ui";
import { tokenizeCommand } from "../util";

/** Mirrors `parse_port_spec` in engine.rs, so the error arrives before the IPC call. */
function portError(spec: string): string | null {
  const trimmed = spec.trim();
  if (!trimmed) return null;
  const colon = trimmed.indexOf(":");
  if (colon === -1) return "must be HOST:CONTAINER";
  const host = trimmed.slice(0, colon);
  const rest = trimmed.slice(colon + 1);
  const slash = rest.indexOf("/");
  const container = slash === -1 ? rest : rest.slice(0, slash);
  const proto = slash === -1 ? "tcp" : rest.slice(slash + 1);

  const numeric = (v: string) => /^\d+$/.test(v) && Number(v) >= 1 && Number(v) <= 65535;
  if (!numeric(host) || !numeric(container)) return "ports must be numbers in 1–65535";
  if (proto !== "tcp" && proto !== "udp") return "protocol must be tcp or udp";
  return null;
}

function envError(spec: string): string | null {
  const trimmed = spec.trim();
  if (!trimmed) return null;
  if (!trimmed.includes("=")) return "must be KEY=VALUE";
  if (trimmed.startsWith("=")) return "missing key";
  return null;
}

function volumeError(spec: string): string | null {
  const trimmed = spec.trim();
  if (!trimmed) return null;
  const parts = trimmed.split(":");
  if (parts.length < 2) return "must be SOURCE:/container/path";
  if (!parts[0]) return "missing source";
  if (!parts[1]?.startsWith("/")) return "container path must be absolute";
  return null;
}

/** A repeatable list of free-text entries with per-row validation. */
function ListField({
  label,
  hint,
  placeholder,
  values,
  onChange,
  validate,
}: {
  label: string;
  hint: string;
  placeholder: string;
  values: string[];
  onChange: (next: string[]) => void;
  validate: (value: string) => string | null;
}) {
  const set = (i: number, v: string) => onChange(values.map((old, j) => (j === i ? v : old)));
  const remove = (i: number) => onChange(values.filter((_, j) => j !== i));

  return (
    <div>
      <div className="mb-1 flex items-baseline justify-between gap-2">
        <label className="text-xs font-medium text-ink">{label}</label>
        <span className="text-[10px] text-ink-faint">{hint}</span>
      </div>
      <div className="space-y-1">
        {values.map((value, i) => {
          const error = validate(value);
          return (
            <div key={i}>
              <div className="flex items-center gap-1">
                <Input
                  className="flex-1"
                  placeholder={placeholder}
                  value={value}
                  onChange={(e) => set(i, e.target.value)}
                />
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => remove(i)}
                  aria-label={`Remove ${label} entry`}
                >
                  ✕
                </Button>
              </div>
              {error && <div className="mt-0.5 pl-1 text-[10px] text-danger">{error}</div>}
            </div>
          );
        })}
        <Button size="sm" variant="ghost" onClick={() => onChange([...values, ""])}>
          + Add
        </Button>
      </div>
    </div>
  );
}

export default function RunImage({
  image,
  onClose,
  onCreated,
}: {
  /** Pre-filled reference, e.g. `nginx:latest`. Editable — the row is a starting point. */
  image: string;
  onClose: () => void;
  /** Fired after a successful create so the caller can refresh and navigate. */
  onCreated?: (id: string, started: boolean) => void;
}) {
  const toast = useToast();

  const [reference, setReference] = useState(image);
  const [name, setName] = useState("");
  const [ports, setPorts] = useState<string[]>([]);
  const [env, setEnv] = useState<string[]>([]);
  const [volumes, setVolumes] = useState<string[]>([]);
  const [network, setNetwork] = useState("");
  const [command, setCommand] = useState("");
  const [autoRemove, setAutoRemove] = useState(false);
  const [restartPolicy, setRestartPolicy] = useState("no");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [networks, setNetworks] = useState<Network[]>([]);
  useEffect(() => {
    let cancelled = false;
    void api
      .listNetworks()
      .then((n) => !cancelled && setNetworks(n))
      .catch(() => {
        /* the field just stays on the default */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const argv = useMemo(() => tokenizeCommand(command), [command]);

  // Docker rejects auto-remove combined with a restart policy; catching it here
  // is clearer than surfacing the daemon's "conflicting options" error.
  const conflict = autoRemove && restartPolicy !== "no";

  const fieldErrors = [
    ...ports.map(portError),
    ...env.map(envError),
    ...volumes.map(volumeError),
  ].filter(Boolean);

  const canSubmit = reference.trim().length > 0 && fieldErrors.length === 0 && !conflict && !busy;

  const submit = async (start: boolean) => {
    setBusy(true);
    setError(null);

    const req: CreateContainerRequest = {
      image: reference.trim(),
      name: name.trim() || null,
      env: env.map((v) => v.trim()).filter(Boolean),
      ports: ports.map((v) => v.trim()).filter(Boolean),
      volumes: volumes.map((v) => v.trim()).filter(Boolean),
      network: network || null,
      command: argv,
      autoRemove,
      restartPolicy: restartPolicy === "no" ? null : restartPolicy,
    };

    let id: string;
    try {
      id = await api.createContainer(req);
    } catch (e) {
      setBusy(false);
      setError(errorMessage(e));
      return;
    }

    if (!start) {
      setBusy(false);
      toast.success(`Created ${name.trim() || id.slice(0, 12)}`);
      onCreated?.(id, false);
      onClose();
      return;
    }

    // Created but not started is a real state, not a failure — say so precisely
    // rather than implying nothing happened.
    try {
      await api.startContainer(id);
      toast.success(`Started ${name.trim() || id.slice(0, 12)}`);
      onCreated?.(id, true);
      onClose();
    } catch (e) {
      setError(`Container was created but failed to start: ${errorMessage(e)}`);
      onCreated?.(id, false);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      onClose={onClose}
      width="max-w-3xl"
      title="Run a container"
      subtitle={reference || undefined}
      footer={
        <div className="flex w-full items-center justify-between gap-3">
          <div className="min-w-0 flex-1 text-xs text-danger">{error}</div>
          <div className="flex items-center gap-2">
            <Button variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="subtle" disabled={!canSubmit} onClick={() => void submit(false)}>
              Create only
            </Button>
            <Button variant="primary" busy={busy} disabled={!canSubmit} onClick={() => void submit(true)}>
              Run
            </Button>
          </div>
        </div>
      }
    >
      <div className="space-y-4 p-5">
        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className="mb-1 block text-xs font-medium text-ink">Image</label>
            <Input
              className="w-full"
              value={reference}
              onChange={(e) => setReference(e.target.value)}
              placeholder="nginx:latest"
            />
          </div>
          <div>
            <label className="mb-1 block text-xs font-medium text-ink">
              Name <span className="font-normal text-ink-faint">(optional)</span>
            </label>
            <Input
              className="w-full"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="auto-generated if blank"
            />
          </div>
        </div>

        <div className="grid grid-cols-2 gap-4">
          <ListField
            label="Ports"
            hint="HOST:CONTAINER[/udp]"
            placeholder="8080:80"
            values={ports}
            onChange={setPorts}
            validate={portError}
          />
          <ListField
            label="Environment"
            hint="KEY=VALUE"
            placeholder="LOG_LEVEL=debug"
            values={env}
            onChange={setEnv}
            validate={envError}
          />
        </div>

        <ListField
          label="Volumes"
          hint="host path or volume name : container path"
          placeholder="/srv/data:/var/lib/data"
          values={volumes}
          onChange={setVolumes}
          validate={volumeError}
        />

        <div>
          <label className="mb-1 block text-xs font-medium text-ink">
            Command <span className="font-normal text-ink-faint">(optional — overrides the image CMD)</span>
          </label>
          <Input
            className="w-full font-mono"
            value={command}
            onChange={(e) => setCommand(e.target.value)}
            placeholder={`sh -c "while true; do date; sleep 5; done"`}
          />
          {argv.length > 0 && (
            <div className="mt-1.5 flex flex-wrap items-center gap-1">
              <span className="text-[10px] text-ink-faint">runs as:</span>
              {argv.map((a, i) => (
                <code
                  key={i}
                  className="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-[10px] text-ink-dim"
                >
                  {a}
                </code>
              ))}
            </div>
          )}
        </div>

        <div className="grid grid-cols-3 items-end gap-3">
          <div>
            <label className="mb-1 block text-xs font-medium text-ink">Network</label>
            <Select className="w-full" value={network} onChange={(e) => setNetwork(e.target.value)}>
              <option value="">default</option>
              {networks.map((n) => (
                <option key={n.id} value={n.name}>
                  {n.name}
                </option>
              ))}
            </Select>
          </div>

          <div>
            <label className="mb-1 block text-xs font-medium text-ink">Restart policy</label>
            <Select
              className="w-full"
              value={restartPolicy}
              disabled={autoRemove}
              onChange={(e) => setRestartPolicy(e.target.value)}
            >
              <option value="no">no</option>
              <option value="on-failure">on-failure</option>
              <option value="always">always</option>
              <option value="unless-stopped">unless-stopped</option>
            </Select>
          </div>

          <label className="flex items-center gap-2 pb-2 text-xs text-ink">
            <input
              type="checkbox"
              checked={autoRemove}
              onChange={(e) => {
                setAutoRemove(e.target.checked);
                if (e.target.checked) setRestartPolicy("no");
              }}
            />
            Remove when it exits
          </label>
        </div>

        {conflict && (
          <div className="text-xs text-danger">
            Auto-remove cannot be combined with a restart policy.
          </div>
        )}
      </div>
    </Modal>
  );
}
