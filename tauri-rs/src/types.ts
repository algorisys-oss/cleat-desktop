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

/**
 * A registry login Cleat found in the runtime's own config.
 *
 * Cleat never stores credentials — it reads what `docker login` / `podman
 * login` already wrote. `source` is where the credential lives; `null` means
 * there is none and the pull will be anonymous. `username` is `null` when a
 * credential helper owns the secret, since reading it would prompt for a
 * keychain unlock.
 */
export interface RegistryLogin {
  registry: string;
  username: string | null;
  source: string | null;
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

// ================================================================ kubernetes
//
// Mirrors src-tauri/src/k8s/. Kept in this file rather than a second one
// because the frontend has always had exactly one types module, but note the
// Rust side does split: a cluster and a container runtime share no vocabulary.

export interface KubeContext {
  name: string;
  cluster: string;
  user: string;
  namespace: string | null;
  current: boolean;
}

/** Whether a context's cluster actually answered. Mirrors RuntimeInfo. */
export interface ClusterInfo {
  context: string;
  available: boolean;
  version: string | null;
  detail: string | null;
}

export interface K8sNamespace {
  name: string;
  phase: string;
  /** Seconds since creation. */
  age: number;
}

export interface K8sPod {
  name: string;
  namespace: string;
  /** What `kubectl get pods` shows: a container reason when one is more
   *  informative than the phase, so CrashLoopBackOff surfaces rather than
   *  "Running". */
  status: string;
  ready: string;
  restarts: number;
  age: number;
  node: string | null;
  ip: string | null;
  containers: string[];
}

export interface K8sDeployment {
  name: string;
  namespace: string;
  ready: string;
  upToDate: number;
  available: number;
  age: number;
  images: string[];
}

export interface K8sService {
  name: string;
  namespace: string;
  type_: string;
  clusterIp: string | null;
  externalIp: string | null;
  ports: string[];
  age: number;
}

export interface K8sNode {
  name: string;
  status: string;
  roles: string[];
  version: string;
  age: number;
  internalIp: string | null;
}

/** What one document in a manifest would do, or did. */
export interface ManifestOutcome {
  kind: string;
  name: string;
  namespace: string | null;
  /** created | configured | unchanged | deleted | failed */
  action: string;
  error: string | null;
}

/** Something the container-to-manifest translation could not carry across. */
export interface GenerateWarning {
  kind: string;
  message: string;
}

export interface GeneratedManifest {
  yaml: string;
  warnings: GenerateWarning[];
}

/** A cluster event — the answer to "why is this pod Pending". */
export interface K8sEvent {
  namespace: string;
  /** Normal | Warning */
  type_: string;
  reason: string;
  message: string;
  /** `Pod/web-abc123` */
  object: string;
  count: number;
  /** Seconds since last seen. */
  age: number;
}

/**
 * A ConfigMap or a Secret.
 *
 * `keys` are key *names* only. Secret values are never fetched — putting
 * cluster credentials into this process would give it something it has no
 * reason to hold, and one screenshot away from a bug report.
 */
export interface K8sConfigEntry {
  name: string;
  namespace: string;
  kind: string;
  type_: string | null;
  keys: string[];
  age: number;
}
