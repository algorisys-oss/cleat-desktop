/**
 * In-app updates.
 *
 * Cleat checks GitHub Releases for a newer version, downloads it in the
 * background, and then *asks*. It never installs behind the user's back: an
 * install swaps the running application and needs a restart, and doing that to
 * someone mid-way through watching a container's logs is not a favour.
 *
 * What can actually update itself is narrower than what we ship. The AppImage,
 * the Windows installers and the macOS .app can replace themselves; a `.deb` or
 * `.rpm` belongs to the system package manager and the bare executables belong
 * to whoever put them there. Those installs get told where to get the new
 * version rather than being silently left behind — see `docs/updates.md`.
 *
 * Failures are quiet by design. A machine with no network, a proxy that eats
 * the request, or a corporate firewall are all normal, and none of them are
 * worth an error dialog on startup for a check nobody asked for.
 */
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { useCallback, useEffect, useRef, useState } from "react";

/** Where someone on a `.deb`/`.rpm`/bare-binary install goes instead. */
export const RELEASES_URL = "https://github.com/algorisys-oss/cleat-desktop/releases/latest";

export type UpdateState =
  | { status: "idle" }
  | { status: "checking" }
  | { status: "downloading"; version: string; percent: number | null }
  | { status: "ready"; version: string }
  | { status: "installing"; version: string }
  | { status: "current" }
  /** The check or the download failed, or this install cannot replace itself. */
  | { status: "failed"; message: string; canSelfUpdate: boolean };

/**
 * The updater cannot replace a package-managed install, and says so in a way
 * worth passing on rather than swallowing.
 */
function isNotSelfUpdatable(message: string): boolean {
  const m = message.toLowerCase();
  return m.includes("appimage") || m.includes("not supported") || m.includes("no such file");
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * Startup delay before the first check.
 *
 * Long enough that the runtime probes, the first container list and the window
 * paint have all happened first. The update is never urgent; the app being
 * responsive on launch is.
 */
const FIRST_CHECK_DELAY_MS = 8000;

/**
 * How often to look again while the app stays open.
 *
 * Checking only at launch would never fire for the people this app is built for
 * — it is a monitoring window that sits open for days. Six hours is far below
 * the rate a release could plausibly appear and far above anything GitHub would
 * consider traffic.
 */
const RECHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

export function useUpdates() {
  const [state, setState] = useState<UpdateState>({ status: "idle" });
  // Held between "downloaded" and the user saying yes, so install() has
  // something to install. Not state: replacing it must not re-render.
  const pending = useRef<Update | null>(null);
  const busy = useRef(false);

  const run = useCallback(async (manual: boolean) => {
    if (busy.current) return;
    busy.current = true;
    setState({ status: "checking" });
    try {
      const update = await check();
      if (!update) {
        setState({ status: "current" });
        return;
      }

      setState({ status: "downloading", version: update.version, percent: null });
      let total = 0;
      let taken = 0;
      await update.download((event) => {
        if (event.event === "Started") {
          total = event.data.contentLength ?? 0;
          taken = 0;
        } else if (event.event === "Progress") {
          taken += event.data.chunkLength;
        }
        setState({
          status: "downloading",
          version: update.version,
          // A server that sends no content-length leaves the bar indeterminate
          // rather than inventing a number.
          percent: total > 0 ? Math.min(100, Math.round((taken / total) * 100)) : null,
        });
      });

      pending.current = update;
      setState({ status: "ready", version: update.version });
    } catch (e) {
      const text = message(e);
      // An automatic check that fails is not the user's problem; a check they
      // asked for is, and it says why.
      setState(
        manual || isNotSelfUpdatable(text)
          ? { status: "failed", message: text, canSelfUpdate: !isNotSelfUpdatable(text) }
          : { status: "idle" },
      );
    } finally {
      busy.current = false;
    }
  }, []);

  /** Apply the downloaded update and restart into it. */
  const install = useCallback(async () => {
    const update = pending.current;
    if (!update) return;
    setState({ status: "installing", version: update.version });
    try {
      await update.install();
      await relaunch();
    } catch (e) {
      const text = message(e);
      setState({ status: "failed", message: text, canSelfUpdate: !isNotSelfUpdatable(text) });
    }
  }, []);

  /** Put the offer away; the next launch will find it again. */
  const dismiss = useCallback(() => setState({ status: "idle" }), []);

  // Whether a re-check would be pointless: something is already downloaded,
  // downloading, or installing, and finding it again would restart that.
  const settled = state.status === "idle" || state.status === "current";

  // Dev runs against a live GitHub release that is almost always newer than the
  // working tree, which would offer to "update" a development build into the
  // last shipped one. Nothing good is behind that door.
  const enabled = !import.meta.env.DEV;

  // Deliberately two effects. Folding them into one would re-arm the startup
  // timer every time `settled` flipped — that is, a fresh check eight seconds
  // after every check.
  useEffect(() => {
    if (!enabled) return;
    const first = setTimeout(() => void run(false), FIRST_CHECK_DELAY_MS);
    return () => clearTimeout(first);
  }, [enabled, run]);

  useEffect(() => {
    if (!enabled || !settled) return;
    const repeat = setInterval(() => void run(false), RECHECK_INTERVAL_MS);
    return () => clearInterval(repeat);
  }, [enabled, settled, run]);

  return { state, check: () => run(true), install, dismiss };
}
