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
