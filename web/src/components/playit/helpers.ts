import type { ComponentType } from "preact";
import * as Icon from "../icons";
import type { useT } from "../../i18n";
import type {
  PlayitAccountStatus,
  PlayitAttachDisposition,
  PlayitConnectionState,
  ServerPlayitState,
} from "../../types";

export type AgentDraft = { moveTo: string; disable: boolean };

export function errorText(error: unknown, fallback: string): string {
  return error instanceof Error && error.message ? error.message : fallback;
}

export function safeExternalUrl(value: string | null | undefined): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    return url.protocol === "https:" ? url.href : null;
  } catch {
    return null;
  }
}

export function stateLabel(
  state: PlayitConnectionState,
  t: ReturnType<typeof useT>,
): string {
  return t(`playit.states.${state}` as "playit.states.connected");
}

export function accountLabel(
  state: PlayitAccountStatus,
  t: ReturnType<typeof useT>,
): string {
  return t(`playit.accountStates.${state}` as "playit.accountStates.unknown");
}

export function statusTone(
  state: PlayitConnectionState | undefined,
): "good" | "warn" | "bad" | undefined {
  if (state === "connected") return "good";
  if (
    state === "needs_claim" ||
    state === "starting" ||
    state === "reconnecting" ||
    state === "stopping"
  )
    return "warn";
  if (state) return "bad";
  return undefined;
}

export function attachSuccessMessage(
  disposition: PlayitAttachDisposition | null | undefined,
  t: ReturnType<typeof useT>,
): string {
  if (disposition === "reused") return t("playit.tunnelReused");
  if (disposition === "updated") return t("playit.tunnelUpdated");
  return t("playit.tunnelCreated");
}

export function rowStateMeta(
  state: ServerPlayitState | undefined,
  loadError: string | null,
): { StateIcon: ComponentType<{ size?: number }>; tone?: "good" | "warn" | "bad" } {
  if (!state)
    return { StateIcon: loadError ? Icon.X : Icon.Clock, tone: loadError ? "bad" : undefined };
  switch (state) {
    case "connected":
      return { StateIcon: Icon.Check, tone: "good" };
    case "disabled":
      return { StateIcon: Icon.Globe, tone: undefined };
    case "provisioning":
    case "reconnecting":
      return { StateIcon: Icon.Clock, tone: "warn" };
    case "unavailable":
      return { StateIcon: Icon.X, tone: "bad" };
    default:
      return { StateIcon: Icon.Warning, tone: "warn" };
  }
}
