// Typed wrapper over the Tauri command layer.
//
// The Electron app talked to http://localhost:3000 with `fetch`. There is no
// HTTP server here at all: these are IPC calls into the Rust process.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ComposeAction,
  ComposeService,
  Container,
  CreateContainerRequest,
  Image,
  ImageTransferProgress,
  LogLine,
  Network,
  PullProgress,
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

export const stopAllStreams = () => invoke<void>("stop_all_streams");

/** Fires once at startup with the runtime auto-selection result. */
export const onRuntimeReady = (cb: (kind: RuntimeKind | null) => void) =>
  listen<RuntimeKind | null>("runtime:ready", (e) => cb(e.payload));
