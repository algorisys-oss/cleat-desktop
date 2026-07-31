import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { errorMessage } from "./types";

// ============================================================== primitives

type Tone = "ok" | "warn" | "danger" | "idle" | "accent";

const TONE_CLASS: Record<Tone, string> = {
  ok: "bg-ok/15 text-ok border-ok/30",
  warn: "bg-warn/15 text-warn border-warn/30",
  danger: "bg-danger/15 text-danger border-danger/30",
  idle: "bg-surface-3/60 text-ink-dim border-edge",
  accent: "bg-accent/15 text-accent border-accent/30",
};

export function Badge({ tone = "idle", children }: { tone?: Tone; children: ReactNode }) {
  return (
    <span
      className={`inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-xs font-medium whitespace-nowrap ${TONE_CLASS[tone]}`}
    >
      {children}
    </span>
  );
}

export function Dot({ tone = "idle" }: { tone?: Tone }) {
  const color = {
    ok: "bg-ok",
    warn: "bg-warn",
    danger: "bg-danger",
    idle: "bg-ink-faint",
    accent: "bg-accent",
  }[tone];
  return <span className={`inline-block size-2 shrink-0 rounded-full ${color}`} />;
}

type ButtonVariant = "primary" | "ghost" | "danger" | "subtle";

const BUTTON_CLASS: Record<ButtonVariant, string> = {
  primary: "bg-accent text-surface-0 hover:brightness-110 border-transparent font-medium",
  ghost: "bg-transparent text-ink-dim hover:text-ink hover:bg-surface-2 border-transparent",
  subtle: "bg-surface-2 text-ink hover:bg-surface-3 border-edge",
  danger: "bg-transparent text-danger hover:bg-danger/12 border-danger/30",
};

export function Button({
  variant = "subtle",
  size = "md",
  busy = false,
  className = "",
  children,
  ...rest
}: {
  variant?: ButtonVariant;
  size?: "sm" | "md";
  busy?: boolean;
  className?: string;
  children: ReactNode;
} & React.ButtonHTMLAttributes<HTMLButtonElement>) {
  const pad = size === "sm" ? "px-2 py-1 text-xs" : "px-3 py-1.5 text-sm";
  return (
    <button
      {...rest}
      disabled={rest.disabled || busy}
      className={`inline-flex items-center justify-center gap-1.5 rounded-md border transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${pad} ${BUTTON_CLASS[variant]} ${className}`}
    >
      {busy && <Spinner size={size === "sm" ? 11 : 13} />}
      {children}
    </button>
  );
}

/**
 * Light/dark toggle.
 *
 * A bulb rather than a sun/moon because the action is "turn the lights on",
 * which is what the control does — it lights the *app*, not the sky. Filled,
 * rayed, amber and glowing when light is active; a plain outline when dark. The
 * fill and rays carry the state on their own, so the glow and the colour are
 * decoration rather than the only signal — see `.bulb` in styles.css.
 *
 * `currentColor` throughout, so it inherits a contrast-checked ink token rather
 * than introducing a colour of its own. It rests at `ink-dim` rather than the
 * `ink-faint` the status bar around it uses: at 16px in a strip of small grey
 * text, the faint token made it read as decoration and it went unfound.
 */
export function ThemeToggle({
  theme,
  onToggle,
  className = "",
}: {
  theme: "dark" | "light";
  onToggle: () => void;
  className?: string;
}) {
  const light = theme === "light";
  const label = light ? "Switch to dark theme" : "Switch to light theme";

  return (
    <button
      onClick={onToggle}
      title={label}
      aria-label={label}
      aria-pressed={light}
      className={`rounded p-1 transition-colors outline-none hover:bg-surface-2 hover:text-ink focus-visible:ring-1 focus-visible:ring-accent/40 ${
        light ? "text-warn" : "text-ink-dim"
      } ${className}`}
    >
      <svg
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
        className={`bulb ${light ? "bulb-lit" : ""}`}
      >
        {/* Glass */}
        <path
          d="M9 17.5a5.5 5.5 0 0 1-2-4.2 5 5 0 1 1 10 0 5.5 5.5 0 0 1-2 4.2v1.3a1 1 0 0 1-1 1h-4a1 1 0 0 1-1-1z"
          fill={light ? "currentColor" : "none"}
          opacity={light ? 0.22 : 1}
        />
        {/* Base */}
        <path d="M10 21h4" />
        {light && (
          <g opacity="0.9">
            <path d="M12 1.5v1.6" />
            <path d="M4.4 5.4 5.6 6.5" />
            <path d="M19.6 5.4 18.4 6.5" />
            <path d="M2 13.2h1.5" />
            <path d="M20.5 13.2H22" />
          </g>
        )}
      </svg>
    </button>
  );
}

export function Spinner({ size = 14 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      className="animate-spin"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9" stroke="currentColor" strokeWidth="3" fill="none" opacity="0.25" />
      <path
        d="M21 12a9 9 0 0 0-9-9"
        stroke="currentColor"
        strokeWidth="3"
        fill="none"
        strokeLinecap="round"
      />
    </svg>
  );
}

export function Input({
  className = "",
  ...rest
}: React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      {...rest}
      className={`rounded-md border border-edge bg-surface-1 px-2.5 py-1.5 text-sm text-ink outline-none placeholder:text-ink-faint focus:border-accent/60 focus:ring-1 focus:ring-accent/30 ${className}`}
    />
  );
}

export function Select({
  className = "",
  children,
  ...rest
}: React.SelectHTMLAttributes<HTMLSelectElement> & { children: ReactNode }) {
  return (
    <select
      {...rest}
      className={`rounded-md border border-edge bg-surface-1 px-2.5 py-1.5 text-sm text-ink outline-none focus:border-accent/60 ${className}`}
    >
      {children}
    </select>
  );
}

export function Panel({
  title,
  actions,
  children,
  className = "",
}: {
  title?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={`rounded-lg border border-edge bg-surface-1 ${className}`}>
      {(title || actions) && (
        <div className="flex items-center justify-between gap-3 border-b border-edge px-4 py-2.5">
          <div className="text-sm font-medium text-ink">{title}</div>
          <div className="flex items-center gap-2">{actions}</div>
        </div>
      )}
      {children}
    </div>
  );
}

export function EmptyState({
  title,
  hint,
  action,
}: {
  title: string;
  hint?: string;
  action?: ReactNode;
}) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 px-6 py-14 text-center">
      <div className="text-sm font-medium text-ink-dim">{title}</div>
      {hint && <div className="max-w-md text-xs text-ink-faint">{hint}</div>}
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}

export function ErrorNote({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  return (
    <div className="m-4 rounded-md border border-danger/30 bg-danger/10 px-4 py-3">
      <div className="text-sm font-medium text-danger">Something went wrong</div>
      <div className="mt-1 font-mono text-xs break-words text-ink-dim">
        {errorMessage(error)}
      </div>
      {onRetry && (
        <Button size="sm" variant="subtle" className="mt-2" onClick={onRetry}>
          Retry
        </Button>
      )}
    </div>
  );
}

// ================================================================== modal

export function Modal({
  open,
  onClose,
  title,
  subtitle,
  children,
  footer,
  width = "max-w-3xl",
}: {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  subtitle?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  width?: string;
}) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/55 p-6"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className={`flex max-h-full w-full ${width} flex-col overflow-hidden rounded-xl border border-edge bg-surface-1 shadow-2xl`}
      >
        <div className="flex items-start justify-between gap-4 border-b border-edge px-5 py-3">
          <div className="min-w-0">
            <div className="truncate text-sm font-semibold text-ink">{title}</div>
            {subtitle && (
              <div className="mt-0.5 truncate font-mono text-xs text-ink-faint">{subtitle}</div>
            )}
          </div>
          <Button variant="ghost" size="sm" onClick={onClose} aria-label="Close">
            ✕
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-auto">{children}</div>
        {footer && (
          <div className="flex items-center justify-end gap-2 border-t border-edge px-5 py-3">
            {footer}
          </div>
        )}
      </div>
    </div>
  );
}

/** Confirmation for destructive actions, with the target named explicitly. */
export function ConfirmDialog({
  open,
  title,
  body,
  confirmLabel = "Delete",
  busy = false,
  onConfirm,
  onCancel,
  extra,
}: {
  open: boolean;
  title: string;
  body: ReactNode;
  confirmLabel?: string;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  extra?: ReactNode;
}) {
  return (
    <Modal open={open} onClose={onCancel} title={title} width="max-w-md"
      footer={
        <>
          <Button variant="ghost" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button variant="danger" onClick={onConfirm} busy={busy}>
            {confirmLabel}
          </Button>
        </>
      }
    >
      <div className="space-y-3 px-5 py-4 text-sm text-ink-dim">
        {body}
        {extra}
      </div>
    </Modal>
  );
}

// ========================================================== bulk selection

/**
 * Toolbar that appears once rows are selected.
 *
 * It sits below the filter row rather than replacing it: a selection outlives
 * the filter, so "select the nginx ones, then filter to redis and select those
 * too" has to stay possible.
 */
export function BulkBar({
  count,
  noun,
  onClear,
  children,
}: {
  count: number;
  noun: string;
  onClear: () => void;
  children: ReactNode;
}) {
  return (
    <div className="flex items-center gap-2 rounded-md border border-accent/30 bg-accent/10 px-2.5 py-1.5">
      <span className="text-xs font-medium text-accent">
        {count} {noun}
        {count === 1 ? "" : "s"} selected
      </span>
      <div className="flex items-center gap-1">{children}</div>
      <Button size="sm" variant="ghost" className="ml-auto" onClick={onClear}>
        Clear selection
      </Button>
    </div>
  );
}

/** Row / header checkbox. `indeterminate` is DOM-only, hence the ref. */
export function SelectBox({
  checked,
  indeterminate = false,
  onToggle,
  label,
}: {
  checked: boolean;
  indeterminate?: boolean;
  onToggle: (extend: boolean) => void;
  label: string;
}) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = indeterminate && !checked;
  }, [indeterminate, checked]);

  return (
    <input
      ref={ref}
      type="checkbox"
      aria-label={label}
      checked={checked}
      className="accent-accent align-middle"
      // Shift-click extends from the last plain click; the browser would
      // otherwise treat it as an ordinary toggle.
      onClick={(e) => {
        e.stopPropagation();
        onToggle(e.shiftKey);
      }}
      onChange={() => {}}
    />
  );
}

/**
 * What a bulk action actually did.
 *
 * Bulk work is partially successful far more often than not — half the
 * selection stops and the rest were already exited — so the outcome is a list,
 * not a single toast. Only failures are enumerated; the successes are a count.
 */
export function BulkResultDialog({
  open,
  title,
  done,
  verb,
  failures,
  onClose,
}: {
  open: boolean;
  title: string;
  done: number;
  verb: string;
  failures: { label: string; message: string }[];
  onClose: () => void;
}) {
  return (
    <Modal
      open={open}
      onClose={onClose}
      title={title}
      width="max-w-lg"
      footer={
        <Button variant="subtle" onClick={onClose}>
          Close
        </Button>
      }
    >
      <div className="space-y-3 px-5 py-4">
        <div className="text-sm text-ink-dim">
          {done} {verb}, {failures.length} failed.
        </div>
        <div className="max-h-72 space-y-2 overflow-auto">
          {failures.map((f, idx) => (
            <div
              // Labels are not unique — two untagged images share one.
              key={`${f.label}:${idx}`}
              className="rounded border border-danger/30 bg-danger/10 px-3 py-2"
            >
              <div className="text-xs font-medium text-danger">{f.label}</div>
              <div className="mt-0.5 font-mono text-xs break-words text-ink-dim">
                {f.message}
              </div>
            </div>
          ))}
        </div>
      </div>
    </Modal>
  );
}

// ================================================================= toasts

export interface Toast {
  id: number;
  tone: "ok" | "danger" | "accent";
  message: string;
}

const ToastContext = createContext<{
  push: (tone: Toast["tone"], message: string) => void;
  success: (m: string) => void;
  failure: (e: unknown) => void;
}>({ push: () => {}, success: () => {}, failure: () => {} });

export const useToast = () => useContext(ToastContext);

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const seq = useRef(0);

  const push = useCallback((tone: Toast["tone"], message: string) => {
    const id = seq.current++;
    setToasts((t) => [...t, { id, tone, message }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 5000);
  }, []);

  const value = useMemo(
    () => ({
      push,
      success: (m: string) => push("ok", m),
      failure: (e: unknown) => push("danger", errorMessage(e)),
    }),
    [push],
  );

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="pointer-events-none fixed right-4 bottom-4 z-100 flex w-80 flex-col gap-2">
        {toasts.map((t) => (
          <div
            key={t.id}
            className={`pointer-events-auto rounded-md border px-3 py-2 text-xs shadow-lg backdrop-blur ${
              t.tone === "danger"
                ? "border-danger/40 bg-danger/15 text-danger"
                : t.tone === "ok"
                  ? "border-ok/40 bg-ok/15 text-ok"
                  : "border-accent/40 bg-accent/15 text-accent"
            }`}
            onClick={() => setToasts((x) => x.filter((y) => y.id !== t.id))}
          >
            {t.message}
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

// ================================================================== table

export function Table({ children }: { children: ReactNode }) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse text-sm">{children}</table>
    </div>
  );
}

export function Th({
  children,
  className = "",
}: {
  children?: ReactNode;
  className?: string;
}) {
  return (
    <th
      className={`sticky top-0 z-10 border-b border-edge bg-surface-1 px-3 py-2 text-left text-xs font-medium tracking-wide text-ink-faint uppercase ${className}`}
    >
      {children}
    </th>
  );
}

export function Td({
  children,
  className = "",
  ...rest
}: { children?: ReactNode; className?: string } & React.TdHTMLAttributes<HTMLTableCellElement>) {
  return (
    <td {...rest} className={`border-b border-edge/60 px-3 py-2 align-middle ${className}`}>
      {children}
    </td>
  );
}

/** Monospace scroll region used for logs and JSON. */
export function CodeBlock({
  text,
  className = "",
  autoScroll = false,
}: {
  text: string;
  className?: string;
  autoScroll?: boolean;
}) {
  const ref = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (!autoScroll || !ref.current) return;
    const el = ref.current;
    // Only stick to the bottom if the user is already near it, so scrolling
    // back to read something isn't yanked away by the next line.
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    if (nearBottom) el.scrollTop = el.scrollHeight;
  }, [text, autoScroll]);

  return (
    <pre
      ref={ref}
      className={`overflow-auto bg-surface-0 p-4 font-mono text-xs leading-relaxed break-words whitespace-pre-wrap text-ink-dim ${className}`}
    >
      {text}
    </pre>
  );
}
