import { useEffect, useRef, useState } from "preact/hooks";
import { api } from "../api";
import { Banner, Button } from "../components/ui";
import * as Icon from "../components/icons";
import { useDialogs } from "../components/Modal";
import { useToast } from "../components/Toast";
import { useT } from "../i18n";
import type {
  PlayitAccount,
  PlayitAgent,
  PlayitAuthSession,
  PlayitStatus,
  PlayitTunnel,
  Server,
  ServerPlayitView,
} from "../types";
import { ConnectionCard } from "../components/playit/ConnectionCard";
import { AccountCard } from "../components/playit/AccountCard";
import { AgentsCard } from "../components/playit/AgentsCard";
import { ServerTunnelsCard } from "../components/playit/ServerTunnelsCard";
import { TunnelsCard, type CreateTunnelInput } from "../components/playit/TunnelsCard";
import {
  attachSuccessMessage,
  errorText,
  type AgentDraft,
} from "../components/playit/helpers";

export function Playit() {
  const t = useT();
  const toast = useToast();
  const dialogs = useDialogs();

  const [status, setStatus] = useState<PlayitStatus | null>(null);
  const [account, setAccount] = useState<PlayitAccount | null>(null);
  const [tunnels, setTunnels] = useState<PlayitTunnel[]>([]);
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
  const [authBusy, setAuthBusy] = useState(false);
  const authGeneration = useRef(0);
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
    void loadAuth();
    const timer = setInterval(refresh, 5000);
    return () => {
      refreshGeneration.current += 1;
      clearInterval(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function loadAuth() {
    const generation = ++authGeneration.current;
    try {
      const session = await api.playitAuthSession();
      if (generation !== authGeneration.current) return;
      setAuthSession(session);
      setAuthFailure(null);
      if (session.authenticated) {
        try {
          const list = await api.playitAgents();
          if (generation !== authGeneration.current) return;
          setAgents(list);
          setAgentsFailure(null);
        } catch (error) {
          setAgentsFailure(errorText(error, t("errors.playitAction")));
        }
      } else {
        setAgents([]);
      }
    } catch (error) {
      setAuthFailure(errorText(error, t("errors.playitAction")));
    }
  }

  async function login(email: string, password: string) {
    setAuthBusy(true);
    setAuthFailure(null);
    try {
      const session = await api.playitAuthLogin(email, password);
      authGeneration.current += 1;
      setAuthSession(session);
      if (!session.requires_totp) {
        toast.success(t("playit.signedIn"));
        await loadAuth();
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
      const session = await api.playitAuthTotp(code);
      authGeneration.current += 1;
      setAuthSession(session);
      toast.success(t("playit.signedIn"));
      await loadAuth();
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
      authGeneration.current += 1;
      setAuthSession(null);
      setAgents([]);
      toast.success(t("playit.signedOut"));
    } catch (error) {
      setAuthFailure(errorText(error, t("errors.playitAction")));
    } finally {
      setAuthBusy(false);
    }
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
      await loadAuth();
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

      <ConnectionCard
        status={status}
        account={account}
        accountError={accountError}
        claimUrl={claimUrl}
        busy={busy}
        onClaim={() => void claim()}
      />

      <AccountCard
        authFailure={authFailure}
        authSession={authSession}
        authBusy={authBusy}
        busy={busy}
        needsClaim={status?.status === "needs_claim"}
        onLogin={(email, password) => void login(email, password)}
        onTotp={(code) => void submitTotp(code)}
        onLogout={() => void logout()}
        onConnectDirect={() => void connectDirect()}
        onReconnectAgent={() => void reconnectAgent()}
        onDisconnectAgent={() => void disconnectAgent()}
      />

      {authSession?.authenticated && (
        <AgentsCard
          agents={agents}
          agentsFailure={agentsFailure}
          currentAgentId={account?.agent_id ?? null}
          busy={busy}
          onDeleteAgent={(agent, draft) => void deleteAgent(agent, draft)}
        />
      )}

      <ServerTunnelsCard
        servers={servers}
        serverViews={serverViews}
        serverViewErrors={serverViewErrors}
        canConnect={playitConnected}
        busy={busy}
        onConnect={(id) => void connectServer(id)}
        onDisconnect={(server) => void disconnectServer(server)}
        onRepair={(id) => void connectServer(id)}
        onReconcile={(id) => void reconcileServer(id)}
        onForget={(server) => void forgetServer(server)}
        onCopyAddress={(address) => void copyAddress(address)}
      />

      <TunnelsCard
        tunnels={tunnels}
        tunnelError={tunnelError}
        needsClaim={status?.status === "needs_claim"}
        servers={servers}
        busy={busy}
        canCreate={playitConnected}
        onCreate={(input) => void createTunnel(input)}
        onRemove={(tunnel) => void remove(tunnel)}
        onCopyAddress={(address) => void copyAddress(address)}
      />
    </div>
  );
}
