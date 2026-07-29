/**
 * Interactive terminal attached to a process inside a pod.
 *
 * The cluster twin of {@link ExecTerminal}, and deliberately the same shape:
 * xterm owns its DOM node, React must not render into it, and the session
 * handle outlives the effect that opened it.
 *
 * Two differences from the container terminal, both forced by the API:
 *
 * - There is no shell probe. `kubectl exec` has none either; the backend tries
 *   bash, then sh, then busybox's sh and reports which one it got, because a
 *   distroless image has none of them and "no such file" is a worse answer than
 *   "no shell found".
 * - Resize travels on the session's own socket rather than as a separate call,
 *   so it is addressed by channel rather than by an exec id.
 */

import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import * as api from "../api";
import type { K8sPod } from "../types";
import { errorMessage } from "../types";
import { Button, Modal, Select, Spinner } from "../ui";

export default function PodTerminal({ pod, onClose }: { pod: K8sPod; onClose: () => void }) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const sessionRef = useRef<{ channel: string; dispose: () => void } | null>(null);

  const [container, setContainer] = useState<string | null>(pod.containers[0] ?? null);
  const [status, setStatus] = useState<"connecting" | "live" | "closed">("connecting");
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    let disposed = false;
    const term = new Terminal({
      convertEol: false,
      cursorBlink: true,
      fontSize: 13,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      theme: { background: "#00000000" },
      allowProposedApi: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    fit.fit();

    const decoder = new TextDecoder();

    void (async () => {
      try {
        const session = await api.openPodTerminal(
          pod.namespace,
          pod.name,
          container,
          { cols: term.cols, rows: term.rows },
          (chunk) => {
            // Bytes, not text: a multi-byte character split across two frames
            // must survive, and a stream decoder is what makes that work.
            const bytes = Uint8Array.from(atob(chunk), (c) => c.charCodeAt(0));
            term.write(decoder.decode(bytes, { stream: true }));
          },
          (err) => {
            if (disposed) return;
            setStatus("closed");
            if (err) setError(err);
          },
        );

        if (disposed) {
          session.dispose();
          return;
        }
        sessionRef.current = session;
        setStatus("live");
        setError(null);

        term.onData((data) => {
          void api
            .podTerminalWrite(session.channel, btoa(unescape(encodeURIComponent(data))))
            .catch(() => {});
        });
      } catch (e) {
        if (!disposed) {
          setStatus("closed");
          setError(errorMessage(e));
        }
      }
    })();

    const onResize = () => {
      fit.fit();
      const session = sessionRef.current;
      if (session) {
        void api.podTerminalResize(session.channel, term.cols, term.rows).catch(() => {});
      }
    };
    window.addEventListener("resize", onResize);

    return () => {
      disposed = true;
      window.removeEventListener("resize", onResize);
      sessionRef.current?.dispose();
      sessionRef.current = null;
      term.dispose();
    };
  }, [pod.namespace, pod.name, container, attempt]);

  return (
    <Modal
      open
      onClose={onClose}
      title={`Terminal — ${pod.name}`}
      subtitle={pod.namespace}
      width="max-w-5xl"
      footer={
        <>
          <span className="mr-auto flex items-center gap-2 text-xs">
            {status === "connecting" && (
              <>
                <Spinner size={12} />
                <span className="text-ink-faint">connecting…</span>
              </>
            )}
            {status === "live" && <span className="text-ok">connected</span>}
            {status === "closed" && (
              <span className="text-ink-faint">{error ?? "session ended"}</span>
            )}
          </span>
          {status === "closed" && (
            <Button variant="subtle" onClick={() => setAttempt((a) => a + 1)}>
              Reconnect
            </Button>
          )}
          <Button variant="subtle" onClick={onClose}>
            Close
          </Button>
        </>
      }
    >
      <div className="flex flex-col">
        {pod.containers.length > 1 && (
          <div className="flex items-center gap-2 border-b border-edge px-5 py-2">
            <span className="text-xs text-ink-dim">Container</span>
            <Select
              value={container ?? ""}
              onChange={(e) => setContainer(e.target.value)}
              className="text-xs"
            >
              {pod.containers.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </Select>
          </div>
        )}
        <div ref={hostRef} className="h-[60vh] w-full bg-surface-0 p-2" />
      </div>
    </Modal>
  );
}
