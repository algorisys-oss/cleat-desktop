// Typed wrapper over the Tauri command layer.
//
// The Electron app talked to http://localhost:3000 with `fetch`. There is no
// HTTP server here at all: these are IPC calls into the Rust process.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ActivityEntry,
  ComposeAction,
  ComposeService,
  Container,
  CreateContainerRequest,
  Image,
  ImageTransferProgress,
  LogLine,
  Network,
  ClusterInfo,
  GeneratedManifest,
  K8sDeployment,
  K8sNamespace,
  K8sNode,
  K8sPod,
  K8sService,
  KubeContext,
  ManifestOutcome,
  PullProgress,
  RegistryLogin,
  RuntimeInfo,
  RuntimeKind,
  ServiceEntry,
  Stats,
  SystemSummary,
  Volume,
} from "./types";

// ------------------------------------------------------------ runtime/system

export const listRuntimes = () => invoke<RuntimeInfo[]>("list_runtimes");
export const currentRuntime = () => invoke<RuntimeKind | null>("current_runtime");
export const selectRuntime = (kind: RuntimeKind) => invoke<void>("select_runtime", { kind });
export const systemSummary = () => invoke<SystemSummary>("system_summary");
export const pingRuntime = () => invoke<boolean>("ping_runtime");

// ------------------------------------------------------------------ containers

export const listContainers = (all = true) => invoke<Container[]>("list_containers", { all });
export const inspectContainer = (id: string) => invoke<unknown>("inspect_container", { id });
export const startContainer = (id: string) => invoke<void>("start_container", { id });
export const stopContainer = (id: string) => invoke<void>("stop_container", { id });
export const restartContainer = (id: string) => invoke<void>("restart_container", { id });
export const pauseContainer = (id: string) => invoke<void>("pause_container", { id });
export const unpauseContainer = (id: string) => invoke<void>("unpause_container", { id });
export const removeContainer = (id: string, force = true, volumes = false) =>
  invoke<void>("remove_container", { id, force, volumes });
export const createContainer = (req: CreateContainerRequest) =>
  invoke<string>("create_container", { req });
export const containerHealth = (id: string) => invoke<string>("container_health", { id });
export const containerLogs = (id: string, tail = 200, timestamps = false) =>
  invoke<string>("container_logs", { id, tail, timestamps });
export const containerStats = (id: string) => invoke<Stats>("container_stats", { id });
export const listContainerServices = (id: string) =>
  invoke<ServiceEntry[]>("list_container_services", { id });
export const controlContainerService = (id: string, service: string, action: string) =>
  invoke<string>("control_container_service", { id, service, action });

// ---------------------------------------------------------------------- images

export const listImages = () => invoke<Image[]>("list_images");
export const removeImage = (id: string, force = false) =>
  invoke<void>("remove_image", { id, force });
export const inspectImage = (id: string) => invoke<unknown>("inspect_image", { id });
export const imageHistory = (id: string) => invoke<unknown>("image_history", { id });
export const pruneImages = () => invoke<number>("prune_images");
export const registryLogins = () => invoke<RegistryLogin[]>("registry_logins");
/** Point a second reference at an existing image. Cheap; copies nothing. */
export const tagImage = (source: string, target: string) =>
  invoke<void>("tag_image", { source, target });
/** Which registry `image` authenticates against, and as whom. Never a secret. */
export const registryIdentity = (image: string) =>
  invoke<RegistryLogin>("registry_identity", { image });

// -------------------------------------------------------------------- networks

export const listNetworks = () => invoke<Network[]>("list_networks");
export const createNetwork = (name: string, driver: string, internal = false) =>
  invoke<string>("create_network", { name, driver, internal });
export const removeNetwork = (id: string) => invoke<void>("remove_network", { id });
export const inspectNetwork = (id: string) => invoke<unknown>("inspect_network", { id });
export const connectNetwork = (network: string, container: string) =>
  invoke<void>("connect_network", { network, container });
export const disconnectNetwork = (network: string, container: string) =>
  invoke<void>("disconnect_network", { network, container });

// --------------------------------------------------------------------- volumes

export const listVolumes = () => invoke<Volume[]>("list_volumes");
export const createVolume = (name: string, driver?: string) =>
  invoke<Volume>("create_volume", { name, driver: driver ?? null });
export const removeVolume = (name: string, force = false) =>
  invoke<void>("remove_volume", { name, force });
export const inspectVolume = (name: string) => invoke<unknown>("inspect_volume", { name });
export const pruneVolumes = () => invoke<number>("prune_volumes");

// --------------------------------------------------------------------- compose

export const composeServices = (projectDir: string) =>
  invoke<ComposeService[]>("compose_services", { projectDir });
export const composeUp = (projectDir: string) => invoke<string>("compose_up", { projectDir });
export const composeDown = (projectDir: string) => invoke<string>("compose_down", { projectDir });
export const composeRestart = (projectDir: string, service?: string) =>
  invoke<string>("compose_restart", { projectDir, service: service ?? null });

// ------------------------------------------------------------------- streaming
//
// Each subscribe* helper opens a channel, wires listeners, and returns a single
// dispose function. Callers must invoke it (React effect cleanup) or the Rust
// task keeps pumping into a channel nobody reads.

let channelSeq = 0;
const nextChannel = (prefix: string) => `${prefix}-${Date.now()}-${channelSeq++}`;

interface StreamEnd {
  channel: string;
  error: string | null;
}

async function openStream<T>(
  prefix: string,
  start: (channel: string) => Promise<void>,
  stop: () => Promise<unknown>,
  onData: (item: T) => void,
  onEnd?: (error: string | null) => void,
): Promise<() => void> {
  const channel = nextChannel(prefix);
  const unlisteners: UnlistenFn[] = [];
  let disposed = false;

  unlisteners.push(await listen<T>(channel, (e) => onData(e.payload)));
  unlisteners.push(
    await listen<StreamEnd>(`${channel}:end`, (e) => onEnd?.(e.payload.error)),
  );

  try {
    await start(channel);
  } catch (err) {
    unlisteners.forEach((u) => u());
    throw err;
  }

  return () => {
    if (disposed) return;
    disposed = true;
    unlisteners.forEach((u) => u());
    void stop().catch(() => {
      /* the task may already have exited; nothing to recover */
    });
  };
}

export function subscribeLogs(
  id: string,
  onLine: (line: LogLine) => void,
  opts: { tail?: number; onEnd?: (error: string | null) => void } = {},
) {
  return openStream<LogLine>(
    "logs",
    (channel) => invoke<void>("follow_logs", { id, tail: opts.tail ?? 200, channel }),
    () => invoke<boolean>("stop_follow_logs", { id }),
    onLine,
    opts.onEnd,
  );
}

export function subscribeStats(
  id: string,
  onSample: (s: Stats) => void,
  onEnd?: (error: string | null) => void,
) {
  return openStream<Stats>(
    "stats",
    (channel) => invoke<void>("stream_stats", { id, channel }),
    () => invoke<boolean>("stop_stream_stats", { id }),
    onSample,
    onEnd,
  );
}

export function subscribePull(
  image: string,
  onProgress: (p: PullProgress) => void,
  onEnd?: (error: string | null) => void,
) {
  return openStream<PullProgress>(
    "pull",
    (channel) => invoke<void>("pull_image", { image, channel }),
    () => invoke<boolean>("stop_pull", { image }),
    onProgress,
    onEnd,
  );
}

/**
 * Push a reference, streaming the daemon's progress.
 *
 * Reports the same shape as a pull, minus `overall` — push events carry no
 * layer id, so there is nothing honest to compute a percentage from.
 */
export function subscribePush(
  image: string,
  onProgress: (p: PullProgress) => void,
  onEnd?: (error: string | null) => void,
) {
  return openStream<PullProgress>(
    "push",
    (channel) => invoke<void>("push_image", { image, channel }),
    () => invoke<boolean>("stop_push", { image }),
    onProgress,
    onEnd,
  );
}

export function subscribeImageCopy(
  image: string,
  to: RuntimeKind,
  onProgress: (p: ImageTransferProgress) => void,
  onEnd?: (error: string | null) => void,
) {
  return openStream<ImageTransferProgress>(
    "copy",
    (channel) => invoke<void>("copy_image", { image, to, channel }),
    () => invoke<boolean>("stop_copy_image", { image, to }),
    onProgress,
    onEnd,
  );
}

/**
 * Run a compose action with live output.
 *
 * The blocking composeUp/composeDown above return nothing until the command
 * finishes, which for `down` means up to 10s per container that ignores
 * SIGTERM. Use this so the UI can show progress instead of a dead spinner.
 */
export function subscribeComposeExec(
  projectDir: string,
  action: ComposeAction,
  onLine: (line: string) => void,
  opts: { service?: string; onEnd?: (error: string | null) => void } = {},
) {
  return openStream<string>(
    "compose",
    (channel) =>
      invoke<void>("compose_exec", {
        projectDir,
        action,
        service: opts.service ?? null,
        channel,
      }),
    () => invoke<boolean>("stop_compose_exec", { projectDir }),
    onLine,
    opts.onEnd,
  );
}

export const stopComposeExec = (projectDir: string) =>
  invoke<boolean>("stop_compose_exec", { projectDir });

// ------------------------------------------------------------------------ exec

/** Ask the container which interactive shell it actually has. */
export const detectShell = (id: string) => invoke<string>("detect_shell", { id });

/**
 * A live terminal session.
 *
 * Unlike the subscribe* helpers this cannot collapse to a single dispose
 * function: exec is two-way, so the caller keeps needing the session handle to
 * push keystrokes and size changes back down.
 */
export interface ExecSession {
  write(data: string): void;
  resize(cols: number, rows: number): void;
  dispose(): void;
}

const encoder = new TextEncoder();

const toBase64 = (bytes: Uint8Array) => {
  let binary = "";
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
  return btoa(binary);
};

const fromBase64 = (b64: string) => {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
};

/**
 * Attach a TTY to a new process inside a running container.
 *
 * Both legs are base64 over the IPC boundary. `onData` therefore hands back raw
 * bytes, not a string: terminal output carries escape sequences and can split a
 * multi-byte character across chunks, so decoding here would corrupt it. Give
 * the bytes to xterm and let it do the decoding statefully.
 */
export async function openExec(
  id: string,
  argv: string[],
  size: { cols: number; rows: number },
  onData: (bytes: Uint8Array) => void,
  onEnd?: (error: string | null) => void,
): Promise<ExecSession> {
  const channel = nextChannel("exec");
  const unlisteners: UnlistenFn[] = [];
  let disposed = false;

  unlisteners.push(await listen<string>(channel, (e) => onData(fromBase64(e.payload))));
  unlisteners.push(
    await listen<StreamEnd>(`${channel}:end`, (e) => onEnd?.(e.payload.error)),
  );

  try {
    await invoke<void>("exec_start", {
      id,
      argv,
      cols: size.cols,
      rows: size.rows,
      channel,
    });
  } catch (err) {
    unlisteners.forEach((u) => u());
    throw err;
  }

  return {
    write(data: string) {
      if (disposed) return;
      // Errors here are almost always "the process just exited", which the end
      // frame reports properly; surfacing them per keystroke would be noise.
      void invoke<void>("exec_write", {
        session: channel,
        data: toBase64(encoder.encode(data)),
      }).catch(() => {});
    },
    resize(cols: number, rows: number) {
      if (disposed) return;
      void invoke<void>("exec_resize", { session: channel, cols, rows }).catch(() => {});
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      unlisteners.forEach((u) => u());
      void invoke<boolean>("exec_stop", { session: channel }).catch(() => {});
    },
  };
}

export const stopAllStreams = () => invoke<void>("stop_all_streams");

/** Fires once at startup with the runtime auto-selection result. */
export const onRuntimeReady = (cb: (kind: RuntimeKind | null) => void) =>
  listen<RuntimeKind | null>("runtime:ready", (e) => cb(e.payload));

// -------------------------------------------------------------------- activity

/** Everything Cleat has asked a runtime to do this session, newest first. */
export const activityLog = () => invoke<ActivityEntry[]>("activity_log");
export const clearActivityLog = () => invoke<void>("clear_activity_log");

// -------------------------------------------------------------- kubernetes

export const k8sContexts = () => invoke<KubeContext[]>("k8s_contexts");
export const k8sProbeClusters = () => invoke<ClusterInfo[]>("k8s_probe_clusters");
export const k8sCurrentContext = () => invoke<string | null>("k8s_current_context");
export const k8sSelectContext = (context: string | null) =>
  invoke<void>("k8s_select_context", { context });

/** `namespace: null` means every namespace, matching `kubectl -A`. */
export const k8sListNamespaces = () => invoke<K8sNamespace[]>("k8s_list_namespaces");
export const k8sListPods = (namespace: string | null) =>
  invoke<K8sPod[]>("k8s_list_pods", { namespace });
export const k8sListDeployments = (namespace: string | null) =>
  invoke<K8sDeployment[]>("k8s_list_deployments", { namespace });
export const k8sListServices = (namespace: string | null) =>
  invoke<K8sService[]>("k8s_list_services", { namespace });
export const k8sListNodes = () => invoke<K8sNode[]>("k8s_list_nodes");

export const k8sInspectPod = (namespace: string, name: string) =>
  invoke<unknown>("k8s_inspect_pod", { namespace, name });
export const k8sDeletePod = (namespace: string, name: string) =>
  invoke<void>("k8s_delete_pod", { namespace, name });
export const k8sPodLogs = (
  namespace: string,
  name: string,
  container: string | null,
  tail = 500,
) => invoke<string>("k8s_pod_logs", { namespace, name, container, tail });

/**
 * Follow a pod's logs. One line per event — unlike container logs there is no
 * stdout/stderr split, because the API server merges them and does not say
 * which was which.
 */
export function subscribePodLogs(
  namespace: string,
  name: string,
  container: string | null,
  onLine: (line: string) => void,
  options: { tail?: number; onEnd?: (error: string | null) => void } = {},
) {
  return openStream<string>(
    "k8s-logs",
    (channel) =>
      invoke<void>("k8s_follow_pod_logs", {
        namespace,
        name,
        container,
        tail: options.tail ?? 500,
        channel,
      }),
    () => invoke<boolean>("k8s_stop_pod_logs", { namespace, name }),
    onLine,
    options.onEnd,
  );
}

/** `dryRun` runs the full admission chain server-side and persists nothing. */
export const k8sApplyManifest = (yaml: string, namespace: string | null, dryRun: boolean) =>
  invoke<ManifestOutcome[]>("k8s_apply_manifest", { yaml, namespace, dryRun });
export const k8sDeleteManifest = (yaml: string, namespace: string | null) =>
  invoke<ManifestOutcome[]>("k8s_delete_manifest", { yaml, namespace });
export const k8sEnsureNamespace = (name: string) =>
  invoke<void>("k8s_ensure_namespace", { name });

export const k8sGenerateFromContainer = (
  id: string,
  namespace: string | null,
  replicas: number,
  includeService: boolean,
) =>
  invoke<GeneratedManifest>("k8s_generate_from_container", {
    id,
    namespace,
    replicas,
    includeService,
  });
export const k8sGenerateFromCompose = (
  project: string,
  namespace: string | null,
  replicas: number,
  includeService: boolean,
) =>
  invoke<GeneratedManifest>("k8s_generate_from_compose", {
    project,
    namespace,
    replicas,
    includeService,
  });
