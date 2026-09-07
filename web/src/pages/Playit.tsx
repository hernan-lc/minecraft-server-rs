import { useEffect, useRef, useState } from "preact/hooks";
import { api } from "../api";
import { Banner, Button } from "../components/ui";
import * as Icon from "../components/icons";
import { useDialogs } from "../components/Modal";
import { useToast } from "../components/Toast";
import { useT } from "../i18n";
import type {
  AgentOwnershipInfo,
  PlayitAccount,
  PlayitAgent,
  PlayitAuthSession,
  PlayitStatus,
  PlayitTunnel,
  Server,
  ServerPlayitView,
  TunnelCatalog,
} from "../types";
import { PlayitOverviewCard } from "../components/playit/PlayitOverviewCard";
import type { ChangeAccountInput } from "../components/playit/ChangeAccountModal";
import { AccountCard } from "../components/playit/AccountCard";
import { AgentsCard } from "../components/playit/AgentsCard";
import { ServerTunnelsCard } from "../components/playit/ServerTunnelsCard";
import { TunnelsCard, type CreateTunnelInput } from "../components/playit/TunnelsCard";
import {
  attachSuccessMessage,
  errorText,
  type AgentDraft,
} from "../components/playit/helpers";

type PlayitTab = "overview" | "servers" | "tunnels" | "account";

const TABS: PlayitTab[] = ["overview", "servers", "tunnels", "account"];

const EMPTY_CATALOG: TunnelCatalog = { available: false, source: "none", tunnels: [] };

export function Playit() {
  const t = useT();
  const toast = useToast();
  const dialogs = useDialogs();

  const [tab, setTab] = useState<PlayitTab>("overview");
  const [status, setStatus] = useState<PlayitStatus | null>(null);
  const [account, setAccount] = useState<PlayitAccount | null>(null);
  const [catalog, setCatalog] = useState<TunnelCatalog>(EMPTY_CATALOG);
  const [servers, setServers] = useState<Server[]>([]);
  const [serverViews, setServerViews] = useState<Record<string, ServerPlayitView>>({});
  const [serverViewErrors, setServerViewErrors] = useState<Record<string, string>>({});
  const [claimUrl, setClaimUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [accountError, setAccountError] = useState<string | null>(null);
  const [tunnelError, setTunnelError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [authSession, setAuthSession] = useState<PlayitAuthSession | null>(null);
  const [authFailure, setAuthFailure] = useState<string | null>(null);
  const [agents, setAgents] = useState<PlayitAgent[]>([]);
  const [agentsFailure, setAgentsFailure] = useState<string | null>(null);
  const [ownership, setOwnership] = useState<AgentOwnershipInfo | null>(null);
  const [ownershipError, setOwnershipError] = useState<string | null>(null);
  const [authBusy, setAuthBusy] = useState(false);
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
      // Stage 1: status and account session first. Everything else depends
      // on the lifecycle they report, so tunnel loading never runs before
      // the Playit state is known.
      const [statusResult, sessionResult] = await Promise.allSettled([
        api.playitStatus(),
        api.playitAuthSession(),
      ]);

      if (generation !== refreshGeneration.current) return;

      if (statusResult.status === "fulfilled") {
        setStatus(statusResult.value);
        setFailed(null);
        if (statusResult.value.status === "connected") setClaimUrl(null);
      } else {
        setFailed(errorText(statusResult.reason, t("errors.loadPlayit")));
      }

      const authenticated =
        sessionResult.status === "fulfilled" && sessionResult.value.authenticated;
      if (sessionResult.status === "fulfilled") {
        setAuthSession(sessionResult.value);
        setAuthFailure(null);
      } else {
        setAuthFailure(errorText(sessionResult.reason, t("errors.playitAction")));
      }

      // Stage 2: state that depends on stage 1. The catalog reports an
      // unavailable (but successful) listing while Playit starts, so a
      // missing secret or claim never surfaces as a tunnel error.
      const [accountResult, catalogResult, serverResult, ownershipResult, agentsResult] =
        await Promise.allSettled([
          api.playitAccount(),
          api.playitTunnels(),
          api.servers(),
          api.playitOwnership(),
          authenticated
            ? api.playitAgents()
            : Promise.resolve([] as PlayitAgent[]),
        ]);

      if (generation !== refreshGeneration.current) return;

      if (accountResult.status === "fulfilled") {
        setAccount(accountResult.value);
        setAccountError(null);
        if (accountResult.value.status === "verified") setClaimUrl(null);
      } else {
        setAccountError(errorText(accountResult.reason, t("errors.loadPlayitAccount")));
      }

      if (catalogResult.status === "fulfilled") {
        setCatalog(catalogResult.value);
        setTunnelError(null);
      } else {
        setTunnelError(errorText(catalogResult.reason, t("errors.loadPlayitTunnels")));
      }

      if (ownershipResult.status === "fulfilled") {
        setOwnership(ownershipResult.value);
        setOwnershipError(null);
      } else {
        setOwnershipError(errorText(ownershipResult.reason, t("errors.playitAction")));
      }

      if (agentsResult.status === "fulfilled") {
        setAgents(authenticated ? agentsResult.value : []);
        setAgentsFailure(null);
      } else {
        setAgentsFailure(errorText(agentsResult.reason, t("errors.playitAction")));
      }

      const serverList =
        serverResult.status === "fulfilled" ? serverResult.value : [];
      if (serverResult.status === "fulfilled") {
        setServers(serverList);
      } else {
        setFailed(errorText(serverResult.reason, t("errors.loadServers")));
      }

      // Per-server Playit states drive the repair actions in the server
      // list, so they are reloaded with the same generation guard.
      if (serverList.length > 0) {
        const viewResults = await Promise.allSettled(
          serverList.map((server) => api.serverPlayit(server.id)),
        );
        if (generation !== refreshGeneration.current) return;
        const nextViews: Record<string, ServerPlayitView> = {};
        const nextErrors: Record<string, string> = {};
        viewResults.forEach((result, index) => {
          const id = serverList[index].id;
          if (result.status === "fulfilled") nextViews[id] = result.value;
          else nextErrors[id] = errorText(result.reason, t("errors.playitAction"));
        });
        setServerViews(nextViews);
        setServerViewErrors(nextErrors);
      } else {
        if (generation !== refreshGeneration.current) return;
        setServerViews({});
        setServerViewErrors({});
      }
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function login(email: string, password: string) {
    setAuthBusy(true);
    setAuthFailure(null);
    try {
      const session = await api.playitAuthLogin(email, password);
      setAuthSession(session);
      if (!session.requires_totp) {
        toast.success(t("playit.signedIn"));
        await refresh();
      }
    } catch (error) {
      setAuthFailure(errorText(error, t("playit.signInFailed")));
    } finally {
      setAuthBusy(false);
    }
  }

  async function submitTotp(code: string) {
    setAuthBusy(true);
    setAuthFailure(null);
    try {
      const result = await api.playitAuthTotp(code);
      setAuthSession(result.session);
      if (result.setup) {
        // A TOTP-pending account change continued automatically: its
        // setup already ran and its servers were reconciled.
        if (result.setup.connected) {
          toast.success(t("playit.setupDone"));
        } else {
          toast.success(result.setup.message ?? t("playit.setupPending"));
        }
        if (result.servers_total > 0) {
          toast.info(
            t("playit.serversRecovered", {
              recovered: result.servers_recovered,
              total: result.servers_total,
            }),
          );
        }
      } else if (result.setup_error) {
        toast.error(result.setup_error);
      } else {
        toast.success(t("playit.signedIn"));
      }
      await refresh();
    } catch (error) {
      setAuthFailure(errorText(error, t("playit.signInFailed")));
    } finally {
      setAuthBusy(false);
    }
  }

  async function logout() {
    setAuthBusy(true);
    try {
      await api.playitAuthLogout();
      setAuthSession(null);
      setAgents([]);
      setOwnership(null);
      toast.success(t("playit.signedOut"));
      await refresh();
    } catch (error) {
      setAuthFailure(errorText(error, t("errors.playitAction")));
    } finally {
      setAuthBusy(false);
    }
  }

  async function changeAccount(input: ChangeAccountInput) {
    const result = await api.playitAuthChange(input.email, input.password, {
      ...(input.name ? { name: input.name } : {}),
      ...(input.acknowledge ? { acknowledge_managed_agent: true } : {}),
    });
    setAuthSession(result.session);
    if (result.setup === null) {
      // The new account needs a TOTP code: land on the account tab where
      // the verification dialog opens automatically.
      toast.info(t("playit.totpPendingAfterChange"));
      setTab("account");
    } else if (result.setup.connected) {
      toast.success(t("playit.setupDone"));
    } else {
      toast.success(result.setup.message ?? t("playit.setupPending"));
    }
    toast.success(t("playit.accountChanged"));
    if (result.servers_total > 0) {
      toast.info(
        t("playit.serversRecovered", {
          recovered: result.servers_recovered,
          total: result.servers_total,
        }),
      );
    }
    await refresh();
  }

  async function connectDirect() {
    setBusy(true);
    try {
      const result = await api.playitSetupDirect();
      if (result.connected) {
        toast.success(t("playit.setupDone"));
      } else {
        toast.success(result.message ?? t("playit.setupPending"));
      }
      await refresh();
    } catch (error) {
      toast.error(errorText(error, t("errors.playitAction")));
    } finally {
      setBusy(false);
    }
  }

  async function disconnectAgent() {
    const confirmed = await dialogs.confirm({
      title: t("playit.disconnectAgentTitle"),
      body: t("playit.disconnectAgentBody"),
      danger: true,
    });
    if (!confirmed) return;
    setBusy(true);
    try {
      await api.playitAgentDisconnect();
      toast.success(t("playit.agentDisconnected"));
      await refresh();
    } catch (error) {
      toast.error(errorText(error, t("errors.playitAction")));
    } finally {
      setBusy(false);
    }
  }

  async function reconnectAgent() {
    setBusy(true);
    try {
      await api.playitAgentReconnect();
      toast.success(t("playit.agentReconnected"));
      await refresh();
    } catch (error) {
      toast.error(errorText(error, t("errors.playitAction")));
    } finally {
      setBusy(false);
    }
  }

  async function deleteAgent(agent: PlayitAgent, draft: AgentDraft) {
    const targetName = draft.moveTo
      ? (agents.find((candidate) => candidate.id === draft.moveTo)?.name ?? draft.moveTo)
      : null;
    const confirmed = await dialogs.confirm({
      title: t("playit.deleteAgentTitle", { name: agent.name }),
      body: targetName
        ? t("playit.deleteAgentMoveBody", { target: targetName })
        : t("playit.deleteAgentUnassignBody"),
      danger: true,
    });
    if (!confirmed) return;
    setBusy(true);
    try {
      await api.playitDeleteAgent(agent.id, draft.moveTo || null, draft.disable);
      toast.success(t("playit.agentDeleted"));
      // Deleting an agent reassigns or unassigns its tunnels remotely, so
      // the tunnel and server views must reload to show the new reality.
      await refresh();
    } catch (error) {
      toast.error(errorText(error, t("errors.playitAction")));
    } finally {
      setBusy(false);
    }
  }

  async function createTunnel(input: CreateTunnelInput) {
    setBusy(true);
    try {
      await api.createPlayitTunnel({
        local_port: input.port,
        protocol: input.protocol,
        local_address: input.address,
        name: input.name,
      });
      toast.success(t("playit.tunnelCreated"));
      await refresh();
    } catch (error) {
      toast.error(errorText(error, t("errors.playitAction")));
    } finally {
      setBusy(false);
    }
  }

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

  async function connectServer(id: string) {
    setBusy(true);
    try {
      const view = await api.attachPlayit(id);
      toast.success(attachSuccessMessage(view.disposition, t));
      await refresh();
    } catch (e) {
      toast.error(errorText(e, t("errors.playitAction")));
    } finally {
      setBusy(false);
    }
  }

  async function disconnectServer(server: Server) {
    const confirmed = await dialogs.confirm({
      title: t("playit.deleteTunnelTitle", { name: server.name }),
      body: t("playit.detachTunnelBody"),
      confirmLabel: t("common.delete"),
      danger: true,
    });
    if (!confirmed) return;

    setBusy(true);
    try {
      const result = await api.detachPlayit(server.id);
      if (result.cleanup_pending) toast.info(t("playit.tunnelCleanupPending"));
      else toast.success(t("playit.tunnelDeleted"));
      await refresh();
    } catch (e) {
      toast.error(errorText(e, t("errors.playitAction")));
    } finally {
      setBusy(false);
    }
  }

  async function reconcileServer(id: string) {
    setBusy(true);
    try {
      await api.reconcilePlayit(id);
      toast.success(t("playit.serverReconciled"));
      await refresh();
    } catch (e) {
      toast.error(errorText(e, t("errors.playitAction")));
    } finally {
      setBusy(false);
    }
  }

  async function forgetServer(server: Server) {
    const confirmed = await dialogs.confirm({
      title: t("playit.forgetServer"),
      body: t("playit.forgetServerBody"),
      confirmLabel: t("playit.forgetServer"),
    });
    if (!confirmed) return;

    setBusy(true);
    try {
      await api.forgetPlayit(server.id);
      toast.success(t("playit.serverAssociationForgotten"));
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

  const playitConnected = status?.status === "connected";
  const authenticated = authSession?.authenticated === true;
  const foreignAccount = ownership?.ownership === "different_account";
  const canManageAgentTunnels = playitConnected && !foreignAccount;

  function onTabKeyDown(event: KeyboardEvent) {
    const current = TABS.indexOf(tab);
    let next: number | null = null;
    if (event.key === "ArrowRight") next = (current + 1) % TABS.length;
    else if (event.key === "ArrowLeft") next = (current - 1 + TABS.length) % TABS.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = TABS.length - 1;
    if (next === null) return;
    event.preventDefault();
    const name = TABS[next];
    setTab(name);
    (event.currentTarget as HTMLElement | null)
      ?.querySelector<HTMLElement>(`#playit-tab-${name}`)
      ?.focus();
  }

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

      <div
        role="tablist"
        aria-label={t("playit.title")}
        onKeyDown={onTabKeyDown}
        class="flex gap-1 overflow-x-auto rounded-xl border border-ink-700 bg-ink-850 p-1"
      >
        {TABS.map((name) => {
          const selected = tab === name;
          return (
            <button
              key={name}
              type="button"
              role="tab"
              id={`playit-tab-${name}`}
              aria-selected={selected}
              aria-controls={`playit-panel-${name}`}
              tabIndex={selected ? 0 : -1}
              onClick={() => setTab(name)}
              class={`shrink-0 rounded-lg px-3 py-1.5 text-sm font-medium transition-colors sm:px-4 sm:py-2 ${
                selected
                  ? "bg-ink-600 text-fg"
                  : "text-fg-muted hover:bg-ink-700 hover:text-fg"
              }`}
            >
              {t(`playit.tabs.${name}` as "playit.tabs.overview")}
            </button>
          );
        })}
      </div>

      {tab === "overview" && (
        <div
          role="tabpanel"
          id="playit-panel-overview"
          aria-labelledby="playit-tab-overview"
          class="flex flex-col gap-3 sm:gap-6"
        >
          <PlayitOverviewCard
            status={status}
            account={account}
            authSession={authSession}
            ownership={ownership}
            accountError={accountError}
            ownershipError={ownershipError}
            claimUrl={claimUrl}
            busy={busy}
            onClaim={() => void claim()}
            onConnectDirect={() => void connectDirect()}
            onDisconnectAgent={() => void disconnectAgent()}
            onReconnectAgent={() => void reconnectAgent()}
            onChangeAccount={(input) => changeAccount(input)}
            onOpenAccount={() => setTab("account")}
            onCopyAddress={(address) => void copyAddress(address)}
          />
        </div>
      )}

      {tab === "servers" && (
        <div
          role="tabpanel"
          id="playit-panel-servers"
          aria-labelledby="playit-tab-servers"
        >
          <ServerTunnelsCard
            servers={servers}
            serverViews={serverViews}
            serverViewErrors={serverViewErrors}
            canConnect={playitConnected}
            canManage={canManageAgentTunnels}
            busy={busy}
            onConnect={(id) => void connectServer(id)}
            onDisconnect={(server) => void disconnectServer(server)}
            onRepair={(id) => void connectServer(id)}
            onReconcile={(id) => void reconcileServer(id)}
            onForget={(server) => void forgetServer(server)}
            onCopyAddress={(address) => void copyAddress(address)}
          />
        </div>
      )}

      {tab === "tunnels" && (
        <div
          role="tabpanel"
          id="playit-panel-tunnels"
          aria-labelledby="playit-tab-tunnels"
        >
          <TunnelsCard
            catalog={catalog}
            tunnelError={tunnelError}
            authenticated={authenticated}
            foreignAccount={foreignAccount}
            servers={servers}
            busy={busy}
            canCreate={canManageAgentTunnels}
            onCreate={(input) => void createTunnel(input)}
            onRemove={(tunnel) => void remove(tunnel)}
            onDisconnectServer={(server) => void disconnectServer(server)}
            onCopyAddress={(address) => void copyAddress(address)}
          />
        </div>
      )}

      {tab === "account" && (
        <div
          role="tabpanel"
          id="playit-panel-account"
          aria-labelledby="playit-tab-account"
          class="flex flex-col gap-3 sm:gap-6"
        >
          <AccountCard
            authFailure={authFailure}
            authSession={authSession}
            authBusy={authBusy}
            onLogin={(email, password) => void login(email, password)}
            onTotp={(code) => void submitTotp(code)}
            onLogout={() => void logout()}
          />

          {authenticated && (
            <AgentsCard
              agents={agents}
              agentsFailure={agentsFailure}
              currentAgentId={account?.agent_id ?? null}
              busy={busy}
              onDeleteAgent={(agent, draft) => void deleteAgent(agent, draft)}
            />
          )}
        </div>
      )}
    </div>
  );
}
