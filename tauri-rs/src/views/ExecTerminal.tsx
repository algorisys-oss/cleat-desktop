/**
 * Interactive terminal attached to a process inside a container.
 *
 * The one two-way surface in the app. Everything else streams from the daemon
 * to the UI; here keystrokes travel back, so the session handle has to outlive
 * the effect that opened it.
 *
 * xterm owns the DOM node directly. React must not re-render into it, which is
 * why the terminal lives in a ref and the effect that creates it is keyed only
 * on the identity of the session it represents.
 */

import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import * as api from "../api";
import type { Container } from "../types";
import { Button, Modal, Select, Spinner } from "../ui";

/** Shells worth offering once the probe has named a default. */
const SHELLS = ["/bin/bash", "/bin/sh", "/bin/zsh", "/bin/ash"];

export default function ExecTerminal({
  container,
  onClose,
}: {
  container: Container;
  onClose: () => void;
}) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const sessionRef = useRef<api.ExecSession | null>(null);

  const [shell, setShell] = useState<string | null>(null);
  const [status, setStatus] = useState<"probing" | "connecting" | "live" | "closed">("probing");
  const [error, setError] = useState<string | null>(null);
  // Bumped to force a fresh attach when the user restarts or changes shell.
  const [attempt, setAttempt] = useState(0);

  // Probe once; the answer seeds the picker rather than locking it.
  useEffect(() => {
    let cancelled = false;
    api
      .detectShell(container.id)
      .then((found) => {
        if (!cancelled) {
          setShell(found);
          setStatus("connecting");
        }
      })
      .catch(() => {
        if (!cancelled) {
          setShell("/bin/sh");
          setStatus("connecting");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [container.id]);

  useEffect(() => {
    if (!shell || !hostRef.current) return;
    let cancelled = false;

    const term = new Terminal({
      fontFamily:
        'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace',
      fontSize: 13,
      cursorBlink: true,
      // Matches the app's surface tokens; xterm cannot read CSS variables.
      theme: {
        background: "#0f1115",
        foreground: "#d7dae0",
        cursor: "#d7dae0",
        selectionBackground: "#2f3542",
      },
      // Enough to hold a build log without letting a runaway process grow the
      // buffer forever.
      scrollback: 5000,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(hostRef.current);
    fit.fit();

    api
      .openExec(
        container.id,
        [shell],
        { cols: term.cols, rows: term.rows },
        (bytes) => term.write(bytes),
        (err) => {
          if (cancelled) return;
          setStatus("closed");
          setError(err);
          // Say so in the terminal too — a pane that simply stops accepting
          // input with no explanation reads as a freeze.
          term.write(`\r\n\x1b[2m── session ended${err ? `: ${err}` : ""} ──\x1b[0m\r\n`);
        },
      )
      .then((session) => {
        if (cancelled) {
          session.dispose();
          return;
        }
        sessionRef.current = session;
        setStatus("live");
        setError(null);
        term.onData((data) => session.write(data));
        term.focus();
      })
      .catch((e) => {
        if (cancelled) return;
        setStatus("closed");
        setError(typeof e === "string" ? e : ((e as { message?: string })?.message ?? String(e)));
      });

    // Keep the daemon's idea of the size in step with the pane. Without this,
    // full-screen programs draw into whatever geometry they were given at
    // attach time.
    const observer = new ResizeObserver(() => {
      try {
        fit.fit();
        sessionRef.current?.resize(term.cols, term.rows);
      } catch {
        /* pane detached mid-measure */
      }
    });
    observer.observe(hostRef.current);

    return () => {
      cancelled = true;
      observer.disconnect();
      sessionRef.current?.dispose();
      sessionRef.current = null;
      term.dispose();
    };
  }, [container.id, shell, attempt]);

  const restart = (nextShell?: string) => {
    setStatus("connecting");
    setError(null);
    if (nextShell && nextShell !== shell) setShell(nextShell);
    else setAttempt((n) => n + 1);
  };

  return (
    <Modal
      open
      onClose={onClose}
      width="max-w-5xl"
      title={`Terminal — ${container.name}`}
      subtitle={shell ?? "detecting shell…"}
      footer={
        <div className="flex w-full items-center justify-between gap-3">
          <div className="flex items-center gap-2 text-xs text-ink-faint">
            {status === "probing" && (
              <>
                <Spinner /> detecting shell…
              </>
            )}
            {status === "connecting" && (
              <>
                <Spinner /> attaching…
              </>
            )}
            {status === "live" && <span className="text-ok">connected</span>}
            {status === "closed" && (
              <span className={error ? "text-danger" : undefined}>
                {error ? `disconnected — ${error}` : "session ended"}
              </span>
            )}
          </div>
          <div className="flex items-center gap-2">
            <Select
              value={shell ?? ""}
              onChange={(e) => restart(e.target.value)}
              disabled={!shell}
              aria-label="Shell"
            >
              {/* The probed shell may not be in the static list. */}
              {[...new Set([...(shell ? [shell] : []), ...SHELLS])].map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </Select>
            <Button size="sm" variant="ghost" onClick={() => restart()}>
              Restart
            </Button>
            <Button size="sm" onClick={onClose}>
              Close
            </Button>
          </div>
        </div>
      }
    >
      <div className="bg-[#0f1115] p-2">
        <div ref={hostRef} className="h-[60vh] w-full" />
      </div>
    </Modal>
  );
}
