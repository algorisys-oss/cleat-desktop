export function formatBytes(bytes: number | null | undefined, digits = 1): string {
  if (bytes === null || bytes === undefined) return "—";
  if (bytes < 0) return "—";
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / Math.pow(1024, i);
  return `${value.toFixed(i === 0 ? 0 : digits)} ${units[i]}`;
}

const AGE_UNITS = ["second", "minute", "hour", "day", "week", "month", "year"];
// Divisor to reach the *next* unit: 60s->min, 60min->hr, 24hr->day, 7day->week,
// 4.35week->month, 12month->year.
const AGE_FACTORS = [60, 60, 24, 7, 4.35, 12, Infinity];

/** Unix seconds -> "3 days ago". */
export function formatAge(unixSeconds: number): string {
  if (!unixSeconds) return "—";
  const seconds = Math.floor(Date.now() / 1000) - unixSeconds;
  if (seconds < 1) return "just now";

  let value = seconds;
  let idx = 0;
  while (idx < AGE_FACTORS.length - 1 && value >= AGE_FACTORS[idx]) {
    value /= AGE_FACTORS[idx];
    idx++;
  }
  const rounded = Math.floor(value);
  return `${rounded} ${AGE_UNITS[idx]}${rounded === 1 ? "" : "s"} ago`;
}

export function shortId(id: string, len = 12): string {
  return id.replace(/^sha256:/, "").slice(0, len);
}

export function primaryTag(repoTags: string[]): string {
  const tagged = repoTags.find((t) => t && t !== "<none>:<none>");
  return tagged ?? "<untagged>";
}

/** Group the states Docker reports into the three the UI colours by. */
export function stateTone(state: string): "ok" | "warn" | "danger" | "idle" {
  switch (state) {
    case "running":
      return "ok";
    case "paused":
    case "restarting":
      return "warn";
    case "dead":
      return "danger";
    default:
      return "idle";
  }
}

export function healthTone(health: string): "ok" | "warn" | "danger" | "idle" {
  switch (health) {
    case "healthy":
      return "ok";
    case "starting":
      return "warn";
    case "unhealthy":
      return "danger";
    default:
      return "idle";
  }
}

export function formatPorts(
  ports: { privatePort: number; publicPort: number | null; protocol: string | null }[],
): string {
  if (!ports.length) return "—";
  const seen = new Set<string>();
  const parts: string[] = [];
  for (const p of ports) {
    const text = p.publicPort
      ? `${p.publicPort}→${p.privatePort}/${p.protocol ?? "tcp"}`
      : `${p.privatePort}/${p.protocol ?? "tcp"}`;
    if (!seen.has(text)) {
      seen.add(text);
      parts.push(text);
    }
  }
  return parts.join(", ");
}

/**
 * Split a command line into argv, honouring single and double quotes.
 *
 * Deliberately *not* a shell: no variable expansion, no operators, no globbing.
 * It exists only so a quoted argument survives as one argument. The backend's
 * `command` field is exact argv, so plain whitespace splitting would silently
 * turn `sh -c "while true; do :; done"` into seven arguments with `-c`
 * receiving only `while` — see the comment on CreateContainerRequest.
 *
 * Callers should show the result back to the user rather than relying on it
 * being guessed correctly.
 */
export function tokenizeCommand(input: string): string[] {
  const argv: string[] = [];
  let current = "";
  let quote: '"' | "'" | null = null;
  // Tracked separately from `current` so that `""` yields an empty argument
  // rather than disappearing.
  let started = false;

  for (const ch of input) {
    if (quote) {
      if (ch === quote) quote = null;
      else current += ch;
      continue;
    }
    if (ch === '"' || ch === "'") {
      quote = ch;
      started = true;
      continue;
    }
    if (/\s/.test(ch)) {
      if (started) {
        argv.push(current);
        current = "";
        started = false;
      }
      continue;
    }
    current += ch;
    started = true;
  }
  if (started) argv.push(current);
  return argv;
}
