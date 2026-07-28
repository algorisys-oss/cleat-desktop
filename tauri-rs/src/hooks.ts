import { useCallback, useEffect, useRef, useState } from "react";

export interface AsyncState<T> {
  data: T | null;
  error: unknown;
  loading: boolean;
  /** True only for the first load, so refreshes don't blank the table. */
  initial: boolean;
  reload: () => void;
}

/**
 * Load a value, then poll it.
 *
 * Polling is paused while the window is hidden — the Electron app refreshed on
 * a bare `setInterval`, which kept hammering the daemon with a backgrounded
 * window.
 */
export function usePolled<T>(
  fetcher: () => Promise<T>,
  intervalMs: number,
  deps: unknown[] = [],
): AsyncState<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [loading, setLoading] = useState(true);
  const [initial, setInitial] = useState(true);

  // Keep the latest fetcher without making it a re-subscribe trigger.
  const fetcherRef = useRef(fetcher);
  fetcherRef.current = fetcher;
  const alive = useRef(true);

  const run = useCallback(async () => {
    setLoading(true);
    try {
      const result = await fetcherRef.current();
      if (!alive.current) return;
      setData(result);
      setError(null);
    } catch (e) {
      if (!alive.current) return;
      setError(e);
    } finally {
      if (alive.current) {
        setLoading(false);
        setInitial(false);
      }
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    void run();

    let timer: number | undefined;
    const tick = () => {
      if (document.visibilityState === "visible") void run();
    };
    if (intervalMs > 0) timer = window.setInterval(tick, intervalMs);

    // Catch up immediately when the window comes back into view.
    const onVisible = () => {
      if (document.visibilityState === "visible") void run();
    };
    document.addEventListener("visibilitychange", onVisible);

    return () => {
      alive.current = false;
      if (timer) clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [intervalMs, run, ...deps]);

  return { data, error, loading, initial, reload: run };
}

/** Track which row-level action is in flight, so only that button spins. */
export function useBusyMap() {
  const [busy, setBusy] = useState<Record<string, boolean>>({});

  const run = useCallback(async (key: string, fn: () => Promise<unknown>) => {
    setBusy((b) => ({ ...b, [key]: true }));
    try {
      return await fn();
    } finally {
      setBusy((b) => {
        const next = { ...b };
        delete next[key];
        return next;
      });
    }
  }, []);

  return { busy, run };
}

/** Debounce a value, used for the search boxes. */
export function useDebounced<T>(value: T, ms = 200): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setDebounced(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return debounced;
}

/** Persist a small piece of UI state (last compose dir, filters). */
export function usePersisted<T>(key: string, fallback: T): [T, (v: T) => void] {
  const [value, setValue] = useState<T>(() => {
    try {
      const raw = localStorage.getItem(key);
      return raw ? (JSON.parse(raw) as T) : fallback;
    } catch {
      return fallback;
    }
  });

  const set = useCallback(
    (v: T) => {
      setValue(v);
      try {
        localStorage.setItem(key, JSON.stringify(v));
      } catch {
        /* private mode / quota - not worth surfacing */
      }
    },
    [key],
  );

  return [value, set];
}
