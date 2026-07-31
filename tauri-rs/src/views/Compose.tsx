import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import * as api from "../api";
import { useBusyMap, usePersisted } from "../hooks";
import type { ComposeAction, ComposeService } from "../types";
import { errorMessage } from "../types";
import {
  Badge,
  Button,
  EmptyState,
  Input,
  Panel,
  Spinner,
  Table,
  Td,
  Th,
  useToast,
} from "../ui";
import { shortId } from "../util";

/**
 * Compose stack control.
 *
 * The Electron version took the project directory as free text and passed it
 * into a shell command; here it goes to a native picker and the Rust side
 * canonicalises it before running an argv-form command. A project is named
 * either by its folder or by the compose file itself.
 */
export default function Compose() {
  // The key predates files being accepted, and is kept so an existing project
  // survives the upgrade; what it holds is now either.
  const [projectPath, setProjectPath] = usePersisted<string>("compose.projectDir", "");
  const [draft, setDraft] = useState(projectPath);
  const [services, setServices] = useState<ComposeService[] | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [loading, setLoading] = useState(false);
  const [output, setOutput] = useState<string>("");
  const [runningAction, setRunningAction] = useState<string | null>(null);
  const { busy, run } = useBusyMap();
  const toast = useToast();

  const load = useCallback(
    async (path: string) => {
      if (!path.trim()) {
        setServices(null);
        setError(null);
        return;
      }
      setLoading(true);
      try {
        const list = await api.composeServices(path);
        setServices(list);
        setError(null);
      } catch (e) {
        setError(e);
        setServices(null);
      } finally {
        setLoading(false);
      }
    },
    [],
  );

  useEffect(() => {
    void load(projectPath);
  }, [projectPath, load]);

  // Poll only while a project is loaded, so this view is idle by default.
  useEffect(() => {
    if (!projectPath.trim()) return;
    const t = setInterval(() => {
      if (document.visibilityState === "visible") void load(projectPath);
    }, 6000);
    return () => clearInterval(t);
  }, [projectPath, load]);

  /**
   * Pick a project, either way round.
   *
   * A directory dialog greys files out — which reads as "your compose file is
   * not allowed" — and a file dialog cannot select a folder, so both are
   * offered. Picking the file is also the only way to use one the runtime would
   * never find by name, such as `stack.yml`.
   */
  const browse = async (mode: "folder" | "file") => {
    const options =
      mode === "folder"
        ? { directory: true, multiple: false, title: "Select compose project folder" }
        : {
            multiple: false,
            title: "Select compose file",
            filters: [
              { name: "Compose file", extensions: ["yaml", "yml"] },
              { name: "All files", extensions: ["*"] },
            ],
          };
    try {
      const picked = await open(options);
      if (typeof picked === "string") {
        setDraft(picked);
        setProjectPath(picked);
      }
    } catch (e) {
      toast.failure(e);
    }
  };

  /**
   * Run a compose action, streaming its output.
   *
   * `docker compose down` waits a 10-second SIGTERM grace period per container
   * that ignores it, so a multi-service stack takes tens of seconds. Streaming
   * the output means the user watches it work rather than staring at a spinner
   * and concluding it has hung.
   */
  const action = (action: ComposeAction, label: string, service?: string) => {
    const key = service ? `${action}:${service}` : action;
    setOutput("");
    setRunningAction(key);
    return run(key, async () => {
      // The stream signals completion via its end event, so wait on a promise
      // the listener resolves rather than on the subscribe call itself.
      let settle: (error: string | null) => void = () => {};
      const ended = new Promise<string | null>((resolve) => {
        settle = resolve;
      });

      try {
        const dispose = await api.subscribeComposeExec(
          projectPath,
          action,
          (line) => setOutput((prev) => (prev ? `${prev}\n${line}` : line)),
          { service, onEnd: settle },
        );
        try {
          const failure = await ended;
          if (failure) throw new Error(failure);
          toast.success(label);
        } finally {
          dispose();
        }
      } catch (e) {
        toast.failure(e);
      } finally {
        setRunningAction(null);
        await load(projectPath);
      }
    });
  };

  const cancelAction = async () => {
    try {
      await api.stopComposeExec(projectPath);
      toast.push("accent", "Cancelled");
    } catch (e) {
      toast.failure(e);
    }
  };

  const hasProject = !!projectPath.trim();

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      <Panel title="Compose project">
        <div className="flex flex-wrap items-center gap-2 px-4 py-3">
          <Input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && setProjectPath(draft)}
            placeholder="/path/to/project or /path/to/compose.yaml"
            className="min-w-72 flex-1 font-mono text-xs"
          />
          <Button variant="subtle" size="sm" onClick={() => browse("folder")}>
            Folder…
          </Button>
          <Button variant="subtle" size="sm" onClick={() => browse("file")}>
            File…
          </Button>
          <Button
            variant="subtle"
            size="sm"
            disabled={draft === projectPath}
            onClick={() => setProjectPath(draft)}
          >
            Load
          </Button>
          <div className="ml-auto flex gap-2">
            <Button
              variant="primary"
              size="sm"
              disabled={!hasProject}
              busy={busy.up}
              onClick={() => action("up", "Stack started")}
            >
              Up
            </Button>
            <Button
              variant="subtle"
              size="sm"
              disabled={!hasProject}
              busy={busy.restart}
              onClick={() => action("restart", "Stack restarted")}
            >
              Restart
            </Button>
            <Button
              variant="danger"
              size="sm"
              disabled={!hasProject}
              busy={busy.down}
              onClick={() => action("down", "Stack stopped")}
            >
              Down
            </Button>
          </div>
        </div>
      </Panel>

      <Panel
        title="Services"
        actions={
          <>
            {loading && <Spinner size={12} />}
            <Button size="sm" variant="ghost" onClick={() => load(projectPath)} disabled={!hasProject}>
              Refresh
            </Button>
          </>
        }
        className="min-h-0 flex-1 overflow-auto"
      >
        {!hasProject ? (
          <EmptyState
            title="No project selected"
            hint="Pick a folder holding compose.yaml or docker-compose.yml, or the compose file itself."
            action={
              <div className="flex gap-2">
                <Button variant="primary" size="sm" onClick={() => browse("folder")}>
                  Folder…
                </Button>
                <Button variant="subtle" size="sm" onClick={() => browse("file")}>
                  File…
                </Button>
              </div>
            }
          />
        ) : error ? (
          <div className="m-4 rounded-md border border-warn/30 bg-warn/10 px-4 py-3">
            <div className="text-sm font-medium text-warn">Could not read this project</div>
            <div className="mt-1 font-mono text-xs break-words text-ink-dim">
              {errorMessage(error)}
            </div>
          </div>
        ) : services === null ? (
          <div className="flex justify-center py-16">
            <Spinner size={22} />
          </div>
        ) : services.length === 0 ? (
          <EmptyState
            title="No services running"
            hint="The compose file was found, but nothing from it is up yet."
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>Service</Th>
                <Th>State</Th>
                <Th>Image</Th>
                <Th>Ports</Th>
                <Th>Container</Th>
                <Th className="text-right">Actions</Th>
              </tr>
            </thead>
            <tbody>
              {services.map((s) => (
                <tr key={`${s.project}/${s.name}`} className="hover:bg-surface-2/50">
                  <Td className="font-medium text-ink">{s.name}</Td>
                  <Td>
                    <Badge
                      tone={
                        s.state.startsWith("running") || s.state.startsWith("Up")
                          ? "ok"
                          : s.state.includes("exit")
                            ? "idle"
                            : "warn"
                      }
                    >
                      {s.state || "unknown"}
                    </Badge>
                  </Td>
                  <Td className="max-w-56 truncate text-ink-dim" title={s.image}>
                    {s.image || "—"}
                  </Td>
                  <Td className="font-mono text-xs text-ink-dim">
                    {s.ports.length ? s.ports.join(", ") : "—"}
                  </Td>
                  <Td className="font-mono text-xs text-ink-faint">
                    {s.containerId ? shortId(s.containerId) : "—"}
                  </Td>
                  <Td>
                    <div className="flex justify-end">
                      <Button
                        size="sm"
                        variant="ghost"
                        busy={busy[`restart:${s.name}`]}
                        onClick={() => action("restart", `${s.name} restarted`, s.name)}
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
      </Panel>

      {(output || runningAction) && (
        <Panel
          title={runningAction ? `Running \u2018${runningAction}\u2019\u2026` : "Last command output"}
          actions={
            runningAction ? (
              <>
                <Spinner size={12} />
                <Button size="sm" variant="danger" onClick={cancelAction}>
                  Cancel
                </Button>
              </>
            ) : (
              <Button size="sm" variant="ghost" onClick={() => setOutput("")}>
                Clear
              </Button>
            )
          }
        >
          <pre className="max-h-40 overflow-auto bg-surface-0 p-3 font-mono text-xs whitespace-pre-wrap text-ink-dim">
            {output || "starting\u2026"}
          </pre>
        </Panel>
      )}
    </div>
  );
}
