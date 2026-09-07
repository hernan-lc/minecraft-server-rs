import { useEffect, useRef, useState } from "preact/hooks";
import { api } from "../api";
import {
  Actions,
  Banner,
  Button,
  Card,
  Empty,
  Field,
  Select,
} from "../components/ui";
import * as Icon from "../components/icons";
import { useDialogs } from "../components/Modal";
import { useToast } from "../components/Toast";
import { useT } from "../i18n";
import type {
  PlayitAccount,
  PlayitAccountStatus,
  PlayitAttachDisposition,
  PlayitConnectionState,
  PlayitStatus,
  PlayitTunnel,
  Server,
} from "../types";

export function Playit() {
  const t = useT();
  const toast = useToast();
  const dialogs = useDialogs();

  const [status, setStatus] = useState<PlayitStatus | null>(null);
  const [account, setAccount] = useState<PlayitAccount | null>(null);
  const [tunnels, setTunnels] = useState<PlayitTunnel[]>([]);
  const [servers, setServers] = useState<Server[]>([]);
  const [claimUrl, setClaimUrl] = useState<string | null>(null);
  const [selectedServer, setSelectedServer] = useState("");
  const [failed, setFailed] = useState<string | null>(null);
  const [accountError, setAccountError] = useState<string | null>(null);
  const [tunnelError, setTunnelError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const refreshGeneration = useRef(0);
  const refreshInFlight = useRef<Promise<void> | null>(null);

  async function refresh() {
    const generation = ++refreshGeneration.current;
    const previous = refreshInFlight.current;
    if (previous) {
      await previous;
      // A newer caller owns the next refresh. This call must not start an
      // extra request or commit anything after it has become stale.
      if (generation !== refreshGeneration.current) return;
    }

    setLoading(true);
    const operation = (async () => {
      const results = await Promise.allSettled([
        api.playitStatus(),
        api.playitAccount(),
        api.playitTunnels(),
        api.servers(),
      ]);

      if (generation !== refreshGeneration.current) return;

      const [statusResult, accountResult, tunnelResult, serverResult] = results;

      if (statusResult.status === "fulfilled") {
        setStatus(statusResult.value);
        setFailed(null);
        if (statusResult.value.status === "connected") setClaimUrl(null);
      } else {
        setFailed(errorText(statusResult.reason, t("errors.loadPlayit")));
      }

      if (accountResult.status === "fulfilled") {
        setAccount(accountResult.value);
        setAccountError(null);
        if (accountResult.value.status === "verified") setClaimUrl(null);
      } else {
        setAccountError(errorText(accountResult.reason, t("errors.loadPlayitAccount")));
      }

      if (tunnelResult.status === "fulfilled") {
        setTunnels(tunnelResult.value);
        setTunnelError(null);
      } else {
        setTunnelError(errorText(tunnelResult.reason, t("errors.loadPlayitTunnels")));
      }

      if (serverResult.status === "fulfilled") setServers(serverResult.value);
      else setFailed(errorText(serverResult.reason, t("errors.loadServers")));
    })();
    refreshInFlight.current = operation;
    try {
      await operation;
    } finally {
      if (refreshInFlight.current === operation) {
        refreshInFlight.current = null;
        if (generation === refreshGeneration.current) setLoading(false);
      }
    }
  }

  useEffect(() => {
    void refresh();
    const timer = setInterval(refresh, 5000);
    return () => {
      refreshGeneration.current += 1;
      clearInterval(timer);
    };
  }, []);

  async function claim() {
    setBusy(true);
    try {
      const result = await api.playitClaim();
      setClaimUrl(result.claim_url);
      toast.success(t("playit.claimStarted"));
      await refresh();
    } catch (e) {
      toast.error(errorText(e, t("errors.playitAction")));
    } finally {
      setBusy(false);
    }
  }

  async function attach() {
    if (!selectedServer) return;
    setBusy(true);
    try {
      const view = await api.attachPlayit(selectedServer);
      toast.success(attachSuccessMessage(view.disposition, t));
      setSelectedServer("");
      await refresh();
    } catch (e) {
      toast.error(errorText(e, t("errors.playitAction")));
    } finally {
      setBusy(false);
    }
  }

  async function remove(tunnel: PlayitTunnel) {
    const server = servers.find((candidate) => candidate.playit?.tunnel_id === tunnel.id);
    const name = server?.name ?? tunnel.name ?? tunnel.id;
    const confirmed = await dialogs.confirm({
      title: t("playit.deleteTunnelTitle", { name }),
      body: server ? t("playit.detachTunnelBody") : t("playit.deleteTunnelBody"),
      confirmLabel: t("common.delete"),
      danger: true,
    });
    if (!confirmed) return;

    setBusy(true);
    try {
      const result = server
        ? await api.detachPlayit(server.id)
        : await api.deletePlayitTunnel(tunnel.id);
      const cleanupPending = "cleanup_pending" in result && result.cleanup_pending;
      if (cleanupPending) toast.info(t("playit.tunnelCleanupPending"));
      else toast.success(t("playit.tunnelDeleted"));
      await refresh();
    } catch (e) {
      toast.error(errorText(e, t("errors.playitAction")));
    } finally {
      setBusy(false);
    }
  }

  async function copyAddress(address: string) {
    if (!navigator.clipboard) {
      toast.error(t("playit.copyUnavailable"));
      return;
    }
    try {
      await navigator.clipboard.writeText(address);
      toast.success(t("playit.addressCopied"));
    } catch {
      toast.error(t("playit.copyUnavailable"));
    }
  }

  const availableServers = servers.filter((server) => !server.playit);
  const loginUrl = safeExternalUrl(account?.login_link);
  const claimingComplete = status?.status === "connected" || account?.status === "verified";
  const activeClaimUrl = claimingComplete ? null : claimUrl ?? account?.claim_url;
  const safeClaimUrl = safeExternalUrl(activeClaimUrl);

  return (
    <div class="mx-auto flex w-full max-w-6xl flex-col gap-3 px-3 py-3 sm:gap-6 sm:px-6 sm:py-8">
      <header class="flex items-center justify-between gap-3 rounded-2xl border border-ink-700 bg-ink-850 px-3 py-3 sm:items-end sm:border-0 sm:bg-transparent sm:px-0 sm:py-0">
        <div class="min-w-0">
          <h1 class="truncate text-xl font-semibold sm:text-2xl">{t("playit.title")}</h1>
          <p class="text-sm text-fg-muted">{t("playit.subtitle")}</p>
        </div>
        <Button
          variant="ghost"
          square
          icon={<Icon.Refresh size={19} />}
          aria-label={t("common.refresh")}
          title={t("common.refresh")}
          class="size-11 shrink-0 sm:h-auto sm:w-auto sm:px-4 sm:py-2"
          disabled={loading}
          onClick={() => void refresh()}
        >
          <span class="hidden sm:inline">{t("common.refresh")}</span>
        </Button>
      </header>

      {failed && <Banner kind="error">{failed}</Banner>}

      <Card title={t("playit.connectionSection")} class="overflow-hidden">
        <div class="grid grid-cols-3 gap-2 sm:gap-4">
          <Detail
            label={t("playit.connection")}
            shortLabel={t("playit.connectionShort")}
            value={status ? stateLabel(status.status, t) : t("common.loading")}
            tone={statusTone(status?.status)}
          />
          <Detail
            label={t("playit.version")}
            shortLabel={t("playit.versionShort")}
            value={status?.version ?? t("common.none")}
          />
          <Detail
            label={t("playit.account")}
            shortLabel={t("playit.accountShort")}
            value={account ? accountLabel(account.status, t) : t("common.none")}
          />
        </div>

        {status?.message && <p class="mt-4 text-sm text-fg-muted">{status.message}</p>}
        {account?.agent_id && (
          <p class="mt-3 break-all text-xs text-fg-muted">
            {t("playit.agentId")}: <span class="font-mono text-fg">{account.agent_id}</span>
          </p>
        )}
        {accountError && (
          <div class="mt-3">
            <Banner kind="error">{accountError}</Banner>
          </div>
        )}

        <Actions>
          {status?.status === "needs_claim" && (
            <Button variant="primary" class="w-full sm:w-auto" disabled={busy} onClick={() => void claim()}>
              {busy ? t("playit.startingClaim") : t("playit.connect")}
            </Button>
          )}
          {loginUrl && (
            <a
              class="inline-flex w-full items-center justify-center gap-2 rounded-full bg-ink-700 px-4 py-2 text-sm font-medium text-fg hover:bg-ink-600 sm:w-auto"
              href={loginUrl}
              target="_blank"
              rel="noopener noreferrer"
            >
              {t("playit.openAccount")}
            </a>
          )}
        </Actions>

        {activeClaimUrl && (
          <div class="mt-4 space-y-2">
            <Banner kind="info">
              {t("playit.claimInstructions")} {" "}
              {safeClaimUrl ? (
                <a
                  class="font-medium text-accent underline"
                  href={safeClaimUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  {t("playit.openClaim")}
                </a>
              ) : (
                t("playit.claimLinkUnavailable")
              )}
            </Banner>
          </div>
        )}
      </Card>

      <Card title={t("playit.serverSection")}>
        <p class="mb-3 text-sm leading-relaxed text-fg-muted sm:mb-4">{t("playit.serverExplain")}</p>
        <div class="flex flex-col items-stretch gap-3 sm:flex-row sm:items-end">
          <div class="min-w-0 flex-1">
            <Field label={t("playit.server")}>
              <Select
                value={selectedServer}
                onChange={(event) => setSelectedServer((event.target as HTMLSelectElement).value)}
              >
                <option value="">{t("playit.chooseServer")}</option>
                {availableServers.map((server) => (
                  <option key={server.id} value={server.id}>
                    {server.name} · :{server.port}
                  </option>
                ))}
              </Select>
            </Field>
          </div>
          <Button
            variant="primary"
            icon={<Icon.Plus size={15} />}
            class="w-full sm:w-auto"
            disabled={busy || !selectedServer || status?.status !== "connected"}
            onClick={() => void attach()}
          >
            {t("playit.createServerTunnel")}
          </Button>
        </div>
        {status?.status !== "connected" && (
          <p class="mt-3 text-xs text-fg-muted">{t("playit.connectBeforeTunnel")}</p>
        )}
      </Card>

      <Card title={t("playit.tunnelsSection")}>
        {tunnelError &&
          (status?.status === "needs_claim" ? (
            <Banner kind="info">{t("playit.tunnelsNeedClaim")}</Banner>
          ) : (
            <Banner kind="error">{tunnelError}</Banner>
          ))}
        {tunnels.length === 0 ? (
          <Empty>{t("playit.noTunnels")}</Empty>
        ) : (
          <div class="space-y-2 sm:space-y-0 sm:divide-y sm:divide-ink-700">
            {tunnels.map((tunnel) => {
              const server = servers.find((candidate) => candidate.playit?.tunnel_id === tunnel.id);
              return (
                <article
                  key={tunnel.id}
                  class="rounded-xl border border-ink-700 bg-ink-900/45 p-3 sm:flex sm:items-center sm:justify-between sm:gap-4 sm:rounded-none sm:border-0 sm:bg-transparent sm:px-0 sm:py-4 sm:first:pt-0 sm:last:pb-0"
                >
                  <div class="min-w-0 flex-1 space-y-1">
                    <div class="flex items-center gap-2">
                      <span
                        class={`size-2 shrink-0 rounded-full ${tunnel.disabled ? "bg-amber-400" : "bg-accent"}`}
                        aria-hidden="true"
                      />
                      <p class="truncate font-medium">{server?.name ?? tunnel.name ?? t("playit.unmanaged")}</p>
                    </div>
                    <p class="text-xs text-fg-muted">
                      {tunnel.tunnel_type === "minecraft-java"
                        ? "Minecraft Java"
                        : tunnel.protocol.toUpperCase()}
                      {tunnel.destination && ` · ${tunnel.destination}`}
                      {tunnel.disabled && ` · ${t("playit.disabled")}`}
                    </p>
                    {tunnel.agent_id && (
                      <p class="text-xs text-fg-muted">
                        {t("playit.agent")}: <span class="font-mono">{tunnel.agent_id}</span>
                      </p>
                    )}
                    {tunnel.disabled_reason && (
                      <p class="text-xs text-amber-300">{tunnel.disabled_reason}</p>
                    )}
                  </div>
                  <div class="mt-3 flex min-w-0 items-center gap-2 sm:mt-0 sm:max-w-sm">
                    <div class="flex min-w-0 flex-1 items-center gap-2 rounded-lg border border-ink-700 bg-ink-850 px-3 py-2 text-xs text-fg-muted">
                      <Icon.Link size={14} />
                      <span class="truncate">{tunnel.display_address || t("common.none")}</span>
                    </div>
                    <Button
                      variant="ghost"
                      square
                      icon={<Icon.Copy size={19} />}
                      aria-label={tunnel.display_address || t("playit.copyUnavailable")}
                      title={t("playit.copyAddress")}
                      class="size-11 shrink-0 sm:h-auto sm:w-auto sm:px-3 sm:py-2"
                      disabled={!tunnel.display_address}
                      onClick={() => void copyAddress(tunnel.display_address)}
                    >
                      <span class="hidden sm:inline">{t("playit.copyAddress")}</span>
                    </Button>
                    <Button
                      variant="danger"
                      square
                      icon={<Icon.Trash size={19} />}
                      aria-label={t("common.delete")}
                      title={t("common.delete")}
                      class="size-11 shrink-0 sm:h-auto sm:w-auto sm:px-3 sm:py-2"
                      disabled={busy}
                      onClick={() => void remove(tunnel)}
                    >
                      <span class="hidden sm:inline">{t("common.delete")}</span>
                    </Button>
                  </div>
                </article>
              );
            })}
          </div>
        )}
      </Card>
    </div>
  );
}

function Detail({
  label,
  shortLabel,
  value,
  tone,
}: {
  label: string;
  shortLabel?: string;
  value: string;
  tone?: "good" | "warn" | "bad";
}) {
  const colours = {
    good: "text-accent",
    warn: "text-amber-300",
    bad: "text-red-300",
  };
  return (
    <div class="min-w-0 rounded-xl border border-ink-700 bg-ink-900/60 px-3 py-2.5 sm:rounded-lg sm:px-4 sm:py-3">
      <p class="truncate text-[10px] uppercase tracking-wider text-fg-muted sm:text-xs">
        <span class="sm:hidden">{shortLabel ?? label}</span>
        <span class="hidden sm:inline">{label}</span>
      </p>
      <p class={`mt-1 truncate text-sm font-medium sm:text-base ${tone ? colours[tone] : "text-fg"}`}>{value}</p>
    </div>
  );
}

function stateLabel(
  state: PlayitConnectionState,
  t: ReturnType<typeof useT>,
): string {
  return t(`playit.states.${state}` as "playit.states.connected");
}

function accountLabel(
  state: PlayitAccountStatus,
  t: ReturnType<typeof useT>,
): string {
  return t(`playit.accountStates.${state}` as "playit.accountStates.unknown");
}

function statusTone(state: PlayitConnectionState | undefined): "good" | "warn" | "bad" | undefined {
  if (state === "connected") return "good";
  if (
    state === "needs_claim" ||
    state === "starting" ||
    state === "reconnecting" ||
    state === "stopping"
  ) return "warn";
  if (state) return "bad";
  return undefined;
}

function errorText(error: unknown, fallback: string): string {
  return error instanceof Error && error.message ? error.message : fallback;
}

function attachSuccessMessage(
  disposition: PlayitAttachDisposition | null | undefined,
  t: ReturnType<typeof useT>,
): string {
  if (disposition === "reused") return t("playit.tunnelReused");
  if (disposition === "updated") return t("playit.tunnelUpdated");
  return t("playit.tunnelCreated");
}

function safeExternalUrl(value: string | null | undefined): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    return url.protocol === "https:" ? url.href : null;
  } catch {
    return null;
  }
}
