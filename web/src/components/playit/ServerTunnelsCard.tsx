import type { ComponentChildren, ComponentType } from "preact";
import { Button, Card, Empty, IconButton } from "../ui";
import * as Icon from "../icons";
import { useT } from "../../i18n";
import type {
  Server,
  ServerPlayitView,
} from "../../types";
import { rowStateMeta } from "./helpers";

type RowAction = {
  key: string;
  label: string;
  icon: ComponentChildren;
  variant: "primary" | "ghost" | "danger" | "subtle";
  disabled: boolean;
  onClick: () => void;
};

export function ServerTunnelsCard({
  servers,
  serverViews,
  serverViewErrors,
  canConnect,
  busy,
  onConnect,
  onDisconnect,
  onRepair,
  onReconcile,
  onForget,
  onCopyAddress,
}: {
  servers: Server[];
  serverViews: Record<string, ServerPlayitView>;
  serverViewErrors: Record<string, string>;
  canConnect: boolean;
  busy: boolean;
  onConnect: (id: string) => void;
  onDisconnect: (server: Server) => void;
  onRepair: (id: string) => void;
  onReconcile: (id: string) => void;
  onForget: (server: Server) => void;
  onCopyAddress: (address: string) => void;
}) {
  const t = useT();
  return (
    <Card
      title={t("playit.serverSection")}
      actions={
        <span class="shrink-0 rounded-full bg-ink-700 px-2.5 py-1 text-xs text-fg-muted">
          {servers.length}
        </span>
      }
    >
      <p class="mb-3 text-sm leading-relaxed text-fg-muted">{t("playit.serverExplain")}</p>
      {servers.length === 0 ? (
        <Empty>{t("playit.noServers")}</Empty>
      ) : (
        <ul class="divide-y divide-ink-700">
          {servers.map((server) => (
            <ServerTunnelRow
              key={server.id}
              server={server}
              view={serverViews[server.id]}
              loadError={serverViewErrors[server.id] ?? null}
              canConnect={canConnect}
              busy={busy}
              onConnect={() => onConnect(server.id)}
              onDisconnect={() => onDisconnect(server)}
              onRepair={() => onRepair(server.id)}
              onReconcile={() => onReconcile(server.id)}
              onForget={() => onForget(server)}
              onCopyAddress={onCopyAddress}
            />
          ))}
        </ul>
      )}
      {!canConnect && servers.length > 0 && (
        <p class="mt-3 text-xs text-fg-muted">{t("playit.connectBeforeTunnel")}</p>
      )}
    </Card>
  );
}

/**
 * One server and its Playit tunnel state, with the actions that make sense
 * for that state: connect a new tunnel, repair a drifted one in place,
 * reconcile pending work, or forget a conflicting association.
 */
function ServerTunnelRow({
  server,
  view,
  loadError,
  canConnect,
  busy,
  onConnect,
  onDisconnect,
  onRepair,
  onReconcile,
  onForget,
  onCopyAddress,
}: {
  server: Server;
  view: ServerPlayitView | undefined;
  loadError: string | null;
  canConnect: boolean;
  busy: boolean;
  onConnect: () => void;
  onDisconnect: () => void;
  onRepair: () => void;
  onReconcile: () => void;
  onForget: () => void;
  onCopyAddress: (address: string) => void;
}) {
  const t = useT();
  const state = view?.state;
  const actionName = (action: string) => `${action}: ${server.name}`;

  const actions: RowAction[] = [];
  if (view) {
    const reconcile: RowAction = {
      key: "reconcile",
      label: t("playit.reconcileServer"),
      icon: <Icon.Refresh size={15} />,
      variant: "ghost",
      disabled: busy,
      onClick: onReconcile,
    };
    switch (state) {
      case "disabled":
        actions.push({
          key: "connect",
          label: t("playit.connectServer"),
          icon: <Icon.Plus size={15} />,
          variant: "primary",
          disabled: busy || !canConnect,
          onClick: onConnect,
        });
        break;
      case "connected":
        actions.push({
          key: "disconnect",
          label: t("playit.disconnectServer"),
          icon: <Icon.Trash size={15} />,
          variant: "danger",
          disabled: busy,
          onClick: onDisconnect,
        });
        break;
      case "missing":
      case "drifted":
      case "agent_mismatch":
        actions.push(
          {
            key: "repair",
            label: t("playit.repairServer"),
            icon: <Icon.Restart size={15} />,
            variant: "primary",
            disabled: busy || !canConnect,
            onClick: onRepair,
          },
          reconcile,
        );
        break;
      case "provisioning":
      case "reconnecting":
      case "unavailable":
        actions.push(reconcile);
        break;
      default:
        actions.push({
          key: "forget",
          label: t("playit.forgetServer"),
          icon: <Icon.X size={15} />,
          variant: "subtle",
          disabled: busy,
          onClick: onForget,
        });
    }
    if (view.cleanup_pending && !actions.some((action) => action.key === "reconcile")) {
      actions.push(reconcile);
    }
  }

  const { StateIcon, tone } = rowStateMeta(state, loadError);
  const toneText =
    tone === "good"
      ? "text-accent"
      : tone === "warn"
        ? "text-amber-300"
        : tone === "bad"
          ? "text-red-300"
          : "text-fg-muted";
  const stateText = !view
    ? (loadError ?? t("common.loading"))
    : t(`playit.serverStates.${state}` as "playit.serverStates.disabled");

  return (
    <li class="flex flex-col gap-3 py-3 first:pt-0 last:pb-0 sm:flex-row sm:items-center sm:justify-between">
      <div class="flex min-w-0 flex-1 items-start gap-2.5">
        <span class={`mt-0.5 shrink-0 ${toneText}`} aria-hidden="true">
          <StateIcon size={16} />
        </span>
        <div class="min-w-0 flex-1">
          <p class="truncate text-sm font-medium">
            {server.name} <span class="font-normal text-fg-muted">· :{server.port}</span>
          </p>
          <p class={`mt-0.5 text-xs ${toneText}`}>{stateText}</p>
          {view?.tunnel?.display_address ? (
            <button
              type="button"
              class="mt-1 flex min-w-0 max-w-full items-center gap-1.5 truncate font-mono text-xs text-fg-muted hover:text-fg"
              title={t("playit.copyAddress")}
              onClick={() => onCopyAddress(view.tunnel?.display_address ?? "")}
            >
              <Icon.Link size={13} />
              <span class="truncate">{view.tunnel.display_address}</span>
            </button>
          ) : (
            view?.binding && (
              <p class="mt-1 truncate text-xs text-fg-muted">
                {t("playit.destination")}:{" "}
                <span class="font-mono">
                  {view.binding.local_address}:{view.binding.local_port}
                </span>
              </p>
            )
          )}
          {view?.cleanup_pending && (
            <p class="mt-0.5 text-xs text-sky-100">{t("playit.tunnelCleanupPending")}</p>
          )}
          {view?.message && state !== "connected" && (
            <p class="mt-0.5 line-clamp-2 text-xs text-fg-muted">{view.message}</p>
          )}
          {loadError && !view && <p class="mt-0.5 text-xs text-red-300">{loadError}</p>}
        </div>
      </div>
      <div class="flex shrink-0 items-center gap-2 pl-6 sm:pl-0">
        {view?.tunnel?.display_address && (
          <IconButton
            label={t("playit.copyAddress")}
            icon={<Icon.Copy size={16} />}
            onClick={() => onCopyAddress(view.tunnel?.display_address ?? "")}
          />
        )}
        {actions.map((action) => (
          <Button
            key={action.key}
            variant={action.variant}
            icon={action.icon}
            class="shrink-0 !px-3 !py-1.5 !text-xs"
            disabled={action.disabled}
            aria-label={actionName(action.label)}
            title={actionName(action.label)}
            onClick={action.onClick}
          >
            {action.label}
          </Button>
        ))}
      </div>
    </li>
  );
}

export type { ComponentType };
