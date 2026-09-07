import { useState } from "preact/hooks";
import { Banner, Button, Card, Empty } from "../ui";
import * as Icon from "../icons";
import { useT } from "../../i18n";
import type { PlayitTunnel, Server, TunnelCatalog, TunnelSource } from "../../types";
import { CreateTunnelModal, type CreateTunnelInput } from "./CreateTunnelModal";

export type { CreateTunnelInput };

export function TunnelsCard({
  catalog,
  tunnelError,
  authenticated,
  foreignAccount,
  servers,
  busy,
  canCreate,
  onCreate,
  onRemove,
  onDisconnectServer,
  onCopyAddress,
}: {
  catalog: TunnelCatalog;
  tunnelError: string | null;
  authenticated: boolean;
  foreignAccount: boolean;
  servers: Server[];
  busy: boolean;
  canCreate: boolean;
  onCreate: (input: CreateTunnelInput) => void;
  onRemove: (tunnel: PlayitTunnel) => void;
  onDisconnectServer: (server: Server) => void;
  onCopyAddress: (address: string) => void;
}) {
  const t = useT();
  const [createOpen, setCreateOpen] = useState(false);
  // Global deletes go through the account session, so they need a login
  // even when the agent itself lists tunnels.
  const canDelete = authenticated && !busy;
  const createHint = !canCreate
    ? foreignAccount
      ? t("playit.foreignTooltip")
      : t("playit.createBeforeConnect")
    : undefined;

  return (
    <Card
      title={t("playit.tunnelsSection")}
      actions={
        <Button
          variant="primary"
          icon={<Icon.Plus size={15} />}
          class="shrink-0 !px-3 !py-1.5 !text-xs sm:!text-sm"
          disabled={!canCreate || busy}
          title={createHint}
          onClick={() => setCreateOpen(true)}
        >
          {t("playit.createTunnel")}
        </Button>
      }
    >
      <p class="mb-3 text-xs text-fg-muted">
        {t("playit.tunnelSource")}: {sourceLabel(catalog.source, t)}
        {!authenticated && ` · ${t("playit.signInToManage")}`}
      </p>
      {!canCreate && createHint && (
        <div class="mb-3">
          <Banner kind="info">{createHint}</Banner>
        </div>
      )}
      {!catalog.available ? (
        <Banner kind="info">{t("playit.tunnelsUnavailable")}</Banner>
      ) : (
        tunnelError && <Banner kind="error">{tunnelError}</Banner>
      )}
      {catalog.tunnels.length === 0 ? (
        <Empty>{t("playit.noTunnels")}</Empty>
      ) : (
        <ul class="divide-y divide-ink-700">
          {catalog.tunnels.map((tunnel) => {
            const server = servers.find(
              (candidate) => candidate.playit?.tunnel_id === tunnel.id,
            );
            return (
              <TunnelRow
                key={tunnel.id}
                tunnel={tunnel}
                serverName={server?.name ?? null}
                busy={busy}
                canDelete={canDelete}
                deleteHint={!authenticated ? t("playit.signInToManage") : undefined}
                onRemove={() => onRemove(tunnel)}
                onDisconnectServer={server ? () => onDisconnectServer(server) : null}
                onCopyAddress={onCopyAddress}
              />
            );
          })}
        </ul>
      )}

      {createOpen && (
        <CreateTunnelModal
          busy={busy}
          onClose={() => setCreateOpen(false)}
          onCreate={(input) => {
            setCreateOpen(false);
            onCreate(input);
          }}
        />
      )}
    </Card>
  );
}

function sourceLabel(source: TunnelSource, t: ReturnType<typeof useT>): string {
  if (source === "account") return t("playit.sourceAccount");
  if (source === "agent") return t("playit.sourceAgent");
  return t("playit.sourceUnavailable");
}

function TunnelRow({
  tunnel,
  serverName,
  busy,
  canDelete,
  deleteHint,
  onRemove,
  onDisconnectServer,
  onCopyAddress,
}: {
  tunnel: PlayitTunnel;
  serverName: string | null;
  busy: boolean;
  canDelete: boolean;
  deleteHint?: string;
  onRemove: () => void;
  /** Set when a server manages this tunnel: global delete is refused. */
  onDisconnectServer: (() => void) | null;
  onCopyAddress: (address: string) => void;
}) {
  const t = useT();

  return (
    <li class="flex items-center gap-2 py-2.5 sm:gap-3 sm:py-3">
      <div class="min-w-0 flex-1">
        <div class="flex flex-wrap items-center gap-x-2 gap-y-0.5">
          <p class="truncate text-sm font-medium text-fg sm:text-base">
            {tunnel.name ?? tunnel.display_address}
          </p>
        </div>
        <p class="mt-0.5 truncate font-mono text-xs text-fg-muted sm:text-sm">
          {tunnel.display_address}
        </p>
        <p class="mt-0.5 truncate text-xs text-fg-muted">
          {tunnel.destination}
          {serverName
            ? ` · ${t("playit.managedBy", { name: serverName })}`
            : ""}
        </p>
      </div>
      <div class="flex shrink-0 items-center gap-1">
        <Button
          variant="ghost"
          square
          icon={<Icon.Copy size={16} />}
          aria-label={t("playit.copyAddress")}
          title={t("playit.copyAddress")}
          class="size-9"
          disabled={!tunnel.display_address}
          onClick={() => onCopyAddress(tunnel.display_address)}
        />
        {onDisconnectServer ? (
          <Button
            variant="ghost"
            class="h-9 !px-2.5 !text-xs sm:!text-sm"
            title={t("playit.disconnectServer")}
            disabled={busy}
            onClick={onDisconnectServer}
          >
            {t("playit.disconnectServer")}
          </Button>
        ) : (
          <Button
            variant="ghost"
            square
            icon={<Icon.Trash size={16} />}
            aria-label={t("common.delete")}
            title={deleteHint ?? t("common.delete")}
            class="size-9 hover:!text-red-300"
            disabled={!canDelete || busy}
            onClick={onRemove}
          />
        )}
      </div>
    </li>
  );
}
