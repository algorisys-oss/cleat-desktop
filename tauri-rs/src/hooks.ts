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

export interface Selection {
  readonly selected: ReadonlySet<string>;
  readonly size: number;
  has: (id: string) => boolean;
  /**
   * Toggle one row. Pass the ordered visible ids and `extend` (shift-click) to
   * add everything between the last plain click and this one.
   */
  toggle: (id: string, ordered?: string[], extend?: boolean) => void;
  setMany: (ids: string[], on: boolean) => void;
  clear: () => void;
}

/**
 * Row selection for the tables with bulk actions.
 *
 * `knownIds` is every id currently loaded, not the filtered rows: selections
 * survive typing in the search box, but a container that has been removed —
 * by Cleat or by anything else touching the daemon — drops out on the next
 * poll rather than leaving a bulk action aimed at something that is gone.
 */
export function useSelection(knownIds: string[]): Selection {
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set());
  const anchor = useRef<string | null>(null);

  useEffect(() => {
    setSelected((prev) => {
      if (prev.size === 0) return prev;
      const live = new Set(knownIds);
      const next = new Set([...prev].filter((id) => live.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [knownIds]);

  const toggle = useCallback((id: string, ordered?: string[], extend = false) => {
    const from = anchor.current;
    setSelected((prev) => {
      const next = new Set(prev);
      if (extend && ordered && from && from !== id) {
        const a = ordered.indexOf(from);
        const b = ordered.indexOf(id);
        if (a >= 0 && b >= 0) {
          for (const between of ordered.slice(Math.min(a, b), Math.max(a, b) + 1)) {
            next.add(between);
          }
          return next;
        }
      }
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
    if (!extend) anchor.current = id;
  }, []);

  const setMany = useCallback((ids: string[], on: boolean) => {
    setSelected((prev) => {
      const next = new Set(prev);
      for (const id of ids) {
        if (on) next.add(id);
        else next.delete(id);
      }
      return next;
    });
    anchor.current = null;
  }, []);

  const clear = useCallback(() => {
    setSelected(new Set());
    anchor.current = null;
  }, []);

  const has = useCallback((id: string) => selected.has(id), [selected]);

  return { selected, size: selected.size, has, toggle, setMany, clear };
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
