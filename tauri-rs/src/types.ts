// Mirrors the DTOs in src-tauri/src/model.rs.
// Keep the two in sync: the Rust side serialises camelCase.

export type RuntimeKind = "docker" | "podman";

export interface RuntimeInfo {
  kind: RuntimeKind;
  /** The daemon answered and we can drive it. */
  available: boolean;
  /** Present on this machine at all (binary, socket, or env var). */
  installed: boolean;
  version: string | null;
  apiVersion: string | null;
  detail: string | null;
}

export interface PortBinding {
  ip: string | null;
  privatePort: number;
  publicPort: number | null;
  protocol: string | null;
}

export interface Container {
  id: string;
  name: string;
  names: string[];
  image: string;
  imageId: string;
  command: string | null;
  created: number;
  state: string;
  status: string;
  ports: PortBinding[];
  labels: Record<string, string>;
  health: string;
  composeProject: string | null;
}

export interface Image {
  id: string;
  repoTags: string[];
  repoDigests: string[];
  created: number;
  size: number;
  containers: number;
  dangling: boolean;
}

export interface Network {
  id: string;
  name: string;
  driver: string;
  scope: string;
  internal: boolean;
  attachable: boolean;
  created: string | null;
  containers: string[];
  subnets: string[];
}

export interface Volume {
  name: string;
  driver: string;
  mountpoint: string;
  createdAt: string | null;
  labels: Record<string, string>;
  scope: string | null;
  size: number | null;
}

export interface Stats {
  id: string;
  name: string;
  cpuPercent: number;
  memoryUsage: number;
  memoryLimit: number;
  memoryPercent: number;
  networkRx: number;
  networkTx: number;
  blockRead: number;
  blockWrite: number;
  pids: number;
  timestamp: number;
}

export interface LogLine {
  containerId: string;
  stream: "stdout" | "stderr";
  message: string;
  seq: number;
}

export interface PullProgress {
  image: string;
  id: string | null;
  status: string;
  current: number | null;
  total: number | null;
  overall: number | null;
  done: boolean;
  error: string | null;
}

export interface ImageTransferProgress {
  image: string;
  from: RuntimeKind;
  to: RuntimeKind;
  /** transferring | importing | complete | failed */
  phase: string;
  bytes: number;
  total: number | null;
  overall: number | null;
  status: string | null;
  done: boolean;
  error: string | null;
}

export type ComposeAction = "up" | "down" | "restart" | "stop" | "start";

export interface ComposeService {
  name: string;
  project: string;
  state: string;
  image: string;
  containerId: string;
  ports: string[];
}

export interface SystemSummary {
  runtime: RuntimeKind;
  version: string;
  apiVersion: string;
  os: string;
  arch: string;
  kernel: string | null;
  storageDriver: string | null;
  cpus: number;
  memory: number;
  containersRunning: number;
  containersPaused: number;
  containersStopped: number;
  images: number;
}

export interface ServiceEntry {
  name: string;
  state: string;
}

export interface CreateContainerRequest {
  image: string;
  name?: string | null;
  env: string[];
  ports: string[];
  volumes: string[];
  network?: string | null;
  /** Exact argv, not a shell string: ["sh", "-c", "echo hi && sleep 1"]. */
  command: string[];
  autoRemove: boolean;
  restartPolicy?: string | null;
}

/** Shape the Rust command layer rejects with. */
export interface AppError {
  kind: string;
  message: string;
}

export function isAppError(e: unknown): e is AppError {
  return (
    typeof e === "object" &&
    e !== null &&
    "kind" in e &&
    "message" in e &&
    typeof (e as AppError).message === "string"
  );
}

export function errorMessage(e: unknown): string {
  if (isAppError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

/** Read-only observation vs. state change. The activity panel defaults to writes. */
export type OpKind = "read" | "write";

/**
 * One operation Cleat performed against a runtime.
 *
 * `detail` is the *actual* Engine API request, not a reconstructed CLI command —
 * Cleat speaks the Engine API, so there is no `docker run` behind these to show.
 * Compose entries are the exception and carry the literal argv, because compose
 * really is a subprocess.
 */
export interface ActivityEntry {
  seq: number;
  /** Milliseconds since the Unix epoch. */
  at: number;
  runtime: RuntimeKind;
  op: string;
  kind: OpKind;
  /** The Engine API requests issued, in order; the literal argv for compose. */
  requests: string[];
  args: [string, string][];
  durationMs: number;
  error: string | null;
}
