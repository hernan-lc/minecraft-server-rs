import { useState } from "preact/hooks";
import { Banner, Button, Card, Empty, Field, Input, Select } from "../ui";
import * as Icon from "../icons";
import { Modal } from "../Modal";
import { useT } from "../../i18n";
import type { PlayitTunnel, Server } from "../../types";

export type CreateTunnelInput = {
  port: number;
  protocol: "tcp" | "udp" | "both";
  address: string;
  name?: string;
};

export function TunnelsCard({
  tunnels,
  tunnelError,
  needsClaim,
  servers,
  busy,
  canCreate,
  onCreate,
  onRemove,
  onCopyAddress,
}: {
  tunnels: PlayitTunnel[];
  tunnelError: string | null;
  needsClaim: boolean;
  servers: Server[];
  busy: boolean;
  canCreate: boolean;
  onCreate: (input: CreateTunnelInput) => void;
  onRemove: (tunnel: PlayitTunnel) => void;
  onCopyAddress: (address: string) => void;
}) {
  const t = useT();
  const [createOpen, setCreateOpen] = useState(false);

  return (
    <Card
      title={t("playit.tunnelsSection")}
      actions={
        <Button
          variant="primary"
          icon={<Icon.Plus size={15} />}
          class="shrink-0 !px-3 !py-1.5 !text-xs sm:!text-sm"
          disabled={!canCreate || busy}
          title={!canCreate ? t("playit.createBeforeConnect") : undefined}
          onClick={() => setCreateOpen(true)}
        >
          {t("playit.createTunnel")}
        </Button>
      }
    >
      {tunnelError &&
        (needsClaim ? (
          <Banner kind="info">{t("playit.tunnelsNeedClaim")}</Banner>
        ) : (
          <Banner kind="error">{tunnelError}</Banner>
        ))}
      {tunnels.length === 0 ? (
        <Empty>{t("playit.noTunnels")}</Empty>
      ) : (
        <ul class="divide-y divide-ink-700">
          {tunnels.map((tunnel) => {
            const server = servers.find(
              (candidate) => candidate.playit?.tunnel_id === tunnel.id,
            );
            return (
              <TunnelRow
                key={tunnel.id}
                tunnel={tunnel}
                serverName={server?.name ?? null}
                busy={busy}
                onRemove={() => onRemove(tunnel)}
                onCopyAddress={onCopyAddress}
              />
            );
          })}
        </ul>
      )}
      {!canCreate && (
        <p class="mt-3 text-xs text-fg-muted">{t("playit.createBeforeConnect")}</p>
      )}

      {createOpen && (
        <CreateTunnelModal
          busy={busy}
          onClose={() => setCreateOpen(false)}
          onCreate={(input) => {
            onCreate(input);
            setCreateOpen(false);
          }}
        />
      )}
    </Card>
  );
}

function TunnelRow({
  tunnel,
  serverName,
  busy,
  onRemove,
  onCopyAddress,
}: {
  tunnel: PlayitTunnel;
  serverName: string | null;
  busy: boolean;
  onRemove: () => void;
  onCopyAddress: (address: string) => void;
}) {
  const t = useT();
  return (
    <li class="flex items-center gap-3 py-3 first:pt-0 last:pb-0">
      <span
        class={`size-2 shrink-0 rounded-full ${tunnel.disabled ? "bg-amber-400" : "bg-accent"}`}
        aria-hidden="true"
      />
      <div class="min-w-0 flex-1">
        <p class="truncate text-sm font-medium">
          {serverName ?? tunnel.name ?? t("playit.unmanaged")}
        </p>
        <p class="truncate text-xs text-fg-muted">
          {tunnel.tunnel_type === "minecraft-java"
            ? "Minecraft Java"
            : tunnel.protocol.toUpperCase()}
          {tunnel.destination && ` · ${tunnel.destination}`}
          {tunnel.disabled && ` · ${t("playit.disabled")}`}
        </p>
      </div>
      {tunnel.display_address && (
        <button
          type="button"
          class="hidden min-w-0 items-center gap-1.5 truncate rounded-lg border border-ink-700 bg-ink-900 px-2.5 py-1.5 font-mono text-xs text-fg-muted hover:text-fg md:flex md:max-w-56"
          title={t("playit.copyAddress")}
          onClick={() => onCopyAddress(tunnel.display_address)}
        >
          <Icon.Link size={13} />
          <span class="truncate">{tunnel.display_address}</span>
        </button>
      )}
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
        <Button
          variant="ghost"
          square
          icon={<Icon.Trash size={16} />}
          aria-label={t("common.delete")}
          title={t("common.delete")}
          class="size-9 hover:!text-red-300"
          disabled={busy}
          onClick={onRemove}
        />
      </div>
    </li>
  );
}

export function CreateTunnelModal({
  busy,
  onClose,
  onCreate,
}: {
  busy: boolean;
  onClose: () => void;
  onCreate: (input: CreateTunnelInput) => void;
}) {
  const t = useT();
  const [name, setName] = useState("");
  const [port, setPort] = useState("");
  const [protocol, setProtocol] = useState<"tcp" | "udp" | "both">("tcp");
  const [address, setAddress] = useState("127.0.0.1");
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  function submit(event?: Event) {
    event?.preventDefault();
    const parsed = Number(port);
    if (!Number.isInteger(parsed) || parsed < 1 || parsed > 65535) {
      setError(t("playit.invalidTunnelPort"));
      return;
    }
    setError(null);
    setCreating(true);
    try {
      const trimmed = name.trim();
      onCreate({
        port: parsed,
        protocol,
        address,
        name: trimmed ? trimmed : undefined,
      });
    } finally {
      setCreating(false);
    }
  }

  return (
    <Modal
      title={t("playit.createTunnelTitle")}
      onClose={onClose}
      width="sm"
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy || creating}>
            {t("common.cancel")}
          </Button>
          <Button
            type="submit"
            form="playit-create-tunnel-form"
            variant="primary"
            disabled={busy || creating}
            icon={<Icon.Plus size={15} />}
          >
            {creating ? t("playit.creatingTunnel") : t("playit.createTunnel")}
          </Button>
        </>
      }
    >
      <form id="playit-create-tunnel-form" onSubmit={submit} class="space-y-4">
        <p class="text-sm text-fg-muted">{t("playit.createTunnelExplain")}</p>
        <div class="grid grid-cols-2 gap-3">
          <Field label={t("playit.tunnelPort")}>
            <Input
              inputMode="numeric"
              value={port}
              placeholder="25565"
              disabled={busy || creating}
              onInput={(event) => setPort(event.currentTarget.value)}
            />
          </Field>
          <Field label={t("playit.tunnelProtocol")}>
            <Select
              aria-label={t("playit.tunnelProtocol")}
              value={protocol}
              disabled={busy || creating}
              onInput={(event) =>
                setProtocol(event.currentTarget.value as "tcp" | "udp" | "both")
              }
            >
              <option value="tcp">TCP</option>
              <option value="udp">UDP</option>
              <option value="both">TCP + UDP</option>
            </Select>
          </Field>
        </div>
        {error && (
          <p class="text-xs text-red-300" role="alert">
            {error}
          </p>
        )}
        <Field label={t("playit.tunnelName")}>
          <Input
            value={name}
            maxLength={100}
            disabled={busy || creating}
            onInput={(event) => setName(event.currentTarget.value)}
          />
        </Field>
        <Field label={t("playit.tunnelAddress")}>
          <Select
            aria-label={t("playit.tunnelAddress")}
            value={address}
            disabled={busy || creating}
            onInput={(event) => setAddress(event.currentTarget.value)}
          >
            <option value="127.0.0.1">127.0.0.1</option>
            <option value="::1">::1</option>
          </Select>
        </Field>
      </form>
    </Modal>
  );
}
