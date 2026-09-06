import type { Status } from "./types";

/** Actions exposed by the server power controls for each lifecycle state. */
export interface ServerActionCapabilities {
  start: boolean;
  cancel: boolean;
  stop: boolean;
  restart: boolean;
  kill: boolean;
}

/**
 * Keep the UI's controls aligned with Guardian's lifecycle semantics.
 * Preparing is a cancellable provisioning task, not a running JVM.
 */
export function serverActionCapabilities(status: Status): ServerActionCapabilities {
  switch (status) {
    case "offline":
    case "crashed":
      return { start: true, cancel: false, stop: false, restart: false, kill: false };
    case "preparing":
      return { start: false, cancel: true, stop: false, restart: false, kill: false };
    case "starting":
      return { start: false, cancel: false, stop: true, restart: false, kill: true };
    case "online":
      return { start: false, cancel: false, stop: true, restart: true, kill: true };
    case "stopping":
      return { start: false, cancel: false, stop: false, restart: false, kill: true };
  }
}
