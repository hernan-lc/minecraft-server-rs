import type { ComponentChildren } from "preact";
import { useState } from "preact/hooks";
import { Banner, Button, Card } from "../ui";
import * as Icon from "../icons";
import { useT } from "../../i18n";
import type {
  AgentOwnershipInfo,
  PlayitAccount,
  PlayitAuthSession,
  PlayitStatus,
} from "../../types";
import { ChangeAccountModal, type ChangeAccountInput } from "./ChangeAccountModal";
import { accountLabel, safeExternalUrl, stateLabel, statusTone } from "./helpers";

/**
 * The single Overview card for Playit: connection status, agent identity,
 * runtime ("agent") vs login ("web") account states, ownership, and the
 * actions valid for the current lifecycle. Agent ID and account concepts
 * render exactly once here.
 */
export function PlayitOverviewCard({
  status,
  account,
  authSession,
  ownership,
  accountError,
  ownershipError,
  claimUrl,
  busy,
  onClaim,
  onConnectDirect,
  onDisconnectAgent,
  onReconnectAgent,
  onChangeAccount,
  onOpenAccount,
  onCopyAddress,
}: {
  status: PlayitStatus | null;
  account: PlayitAccount | null;
  authSession: PlayitAuthSession | null;
  ownership: AgentOwnershipInfo | null;
  accountError: string | null;
  ownershipError: string | null;
  claimUrl: string | null;
  busy: boolean;
  onClaim: () => void;
  onConnectDirect: () => void;
  onDisconnectAgent: () => void;
  onReconnectAgent: () => void;
  onChangeAccount: (input: ChangeAccountInput) => Promise<void>;
  onOpenAccount: () => void;
  onCopyAddress: (address: string) => void;
}) {
  const t = useT();
  const [changeOpen, setChangeOpen] = useState(false);
  const authenticated = authSession?.authenticated === true;
  const agentId = account?.agent_id ?? null;
  const lifecycle = status?.status;
  // Change account is relevant whenever an agent exists or a web account
  // is signed in; otherwise signing in is the only sensible step.
  const showChangeAccount = agentId !== null || authenticated;

  return (
    <Card
      title={t("playit.overviewSection")}
      actions={
        showChangeAccount ? (
          <Button
            variant="ghost"
            icon={<Icon.User size={15} />}
            class="shrink-0 !px-3 !py-1.5 !text-xs sm:!text-sm"
            disabled={busy}
            onClick={() => setChangeOpen(true)}
          >
            {t("playit.changeAccount")}
          </Button>
        ) : undefined
      }
    >
      <div class="flex flex-col gap-4">
        <StatusHeader status={status} />

        <AgentIdentity agentId={agentId} onCopyAddress={onCopyAddress} />

        {lifecycle === "needs_claim" && account?.status !== "verified" && (
          <ClaimBlock claimUrl={claimUrl} busy={busy} onClaim={onClaim} />
        )}

        <OverviewStats
          status={status}
          account={account}
          authSession={authSession}
          accountError={accountError}
        />

        <OwnershipRow
          ownership={ownership}
          ownershipError={ownershipError}
          authenticated={authenticated}
          busy={busy}
          onOpenAccount={onOpenAccount}
        />

        <AgentActions
          lifecycle={lifecycle}
          authenticated={authenticated}
          busy={busy}
          onConnectDirect={onConnectDirect}
          onDisconnectAgent={onDisconnectAgent}
          onReconnectAgent={onReconnectAgent}
          onOpenAccount={onOpenAccount}
        />
      </div>

      {changeOpen && (
        <ChangeAccountModal
          onClose={() => setChangeOpen(false)}
          onChangeAccount={onChangeAccount}
        />
      )}
    </Card>
  );
}

const DOT_TONES: Record<string, string> = {
  good: "bg-emerald-400",
  warn: "bg-amber-400",
  bad: "bg-red-400",
};

function StatusHeader({ status }: { status: PlayitStatus | null }) {
  const t = useT();
  const tone = statusTone(status?.status);
  return (
    <p class="flex items-center gap-2 text-base font-semibold text-fg sm:text-lg">
      <span
        aria-hidden="true"
        class={`inline-block size-2.5 rounded-full ${tone ? DOT_TONES[tone] : "bg-ink-500"}`}
      />
      {status ? stateLabel(status.status, t) : t("common.loading")}
    </p>
  );
}

function AgentIdentity({
  agentId,
  onCopyAddress,
}: {
  agentId: string | null;
  onCopyAddress: (address: string) => void;
}) {
  const t = useT();
  return (
    <div class="min-w-0">
      <p class="text-[10px] uppercase tracking-wider text-fg-muted sm:text-xs">
        {t("playit.agentId")}
      </p>
      {agentId ? (
        <p class="mt-1 flex min-w-0 items-center gap-2">
          <span class="min-w-0 flex-1 break-all font-mono text-sm text-fg sm:text-base">
            {agentId}
          </span>
          <Button
            variant="ghost"
            square
            icon={<Icon.Copy size={15} />}
            aria-label={t("playit.copyAddress")}
            title={t("playit.copyAddress")}
            class="size-8 shrink-0"
            onClick={() => onCopyAddress(agentId)}
          />
        </p>
      ) : (
        <p class="mt-1 text-sm text-fg-muted sm:text-base">{t("playit.noAgentId")}</p>
      )}
    </div>
  );
}

function ClaimBlock({
  claimUrl,
  busy,
  onClaim,
}: {
  claimUrl: string | null;
  busy: boolean;
  onClaim: () => void;
}) {
  const t = useT();
  const safeUrl = safeExternalUrl(claimUrl);
  return (
    <div class="flex flex-col gap-2 rounded-xl border border-ink-700 bg-ink-900/60 px-3 py-2.5 sm:rounded-lg sm:px-4 sm:py-3">
      {!safeUrl && (
        <Button
          variant="primary"
          class="self-start"
          disabled={busy}
          onClick={onClaim}
        >
          {busy ? t("playit.startingClaim") : t("playit.connect")}
        </Button>
      )}
      <div class="mt-1">
        <Banner kind="info">
          {t("playit.claimInstructions")}{" "}
          {safeUrl ? (
            <a
              class="font-medium text-accent underline"
              href={safeUrl}
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
    </div>
  );
}

function OverviewStats({
  status,
  account,
  authSession,
  accountError,
}: {
  status: PlayitStatus | null;
  account: PlayitAccount | null;
  authSession: PlayitAuthSession | null;
  accountError: string | null;
}) {
  const t = useT();
  const authenticated = authSession?.authenticated === true;
  const cells: Array<{ label: string; value: string }> = [
    {
      label: t("playit.serviceColumn"),
      value: status ? stateLabel(status.status, t) : t("common.loading"),
    },
    {
      label: t("playit.version"),
      value: status?.version ?? t("common.none"),
    },
    {
      label: t("playit.agentAccount"),
      value: account ? accountLabel(account.status, t) : t("common.none"),
    },
    {
      label: t("playit.webAccount"),
      value: authenticated
        ? t("playit.accountNumber", { id: authSession?.account_id ?? t("common.unknown") })
        : t("playit.webAccountSignedOut"),
    },
  ];
  return (
    <div>
      <dl class="grid grid-cols-2 gap-2 sm:grid-cols-4 sm:gap-4">
        {cells.map((cell) => (
          <div
            key={cell.label}
            class="min-w-0 rounded-xl border border-ink-700 bg-ink-900/60 px-3 py-2.5 sm:rounded-lg sm:px-4 sm:py-3"
          >
            <dt class="truncate text-[10px] uppercase tracking-wider text-fg-muted sm:text-xs">
              {cell.label}
            </dt>
            <dd class="mt-1 truncate text-sm font-medium text-fg sm:text-base">
              {cell.value}
            </dd>
          </div>
        ))}
      </dl>
      {accountError && (
        <div class="mt-3">
          <Banner kind="error">{accountError}</Banner>
        </div>
      )}
    </div>
  );
}

function OwnershipRow({
  ownership,
  ownershipError,
  authenticated,
  busy,
  onOpenAccount,
}: {
  ownership: AgentOwnershipInfo | null;
  ownershipError: string | null;
  authenticated: boolean;
  busy: boolean;
  onOpenAccount: () => void;
}) {
  const t = useT();
  return (
    <div class="min-w-0">
      <p class="text-[10px] uppercase tracking-wider text-fg-muted sm:text-xs">
        {t("playit.ownershipSection")}
      </p>
      <div class="mt-1">
        {!ownership && ownershipError && <Banner kind="error">{ownershipError}</Banner>}
        {!ownership && !ownershipError && (
          <p class="text-sm text-fg-muted">{t("common.loading")}</p>
        )}
        {ownership?.ownership === "matched" && (
          <p class="text-sm text-fg-muted">{t("playit.ownershipMatched")}</p>
        )}
        {ownership?.ownership === "different_account" && (
          <Banner kind="error">{t("playit.ownershipForeign")}</Banner>
        )}
        {ownership?.ownership === "unknown" && (
          <div class="flex flex-wrap items-center gap-2">
            <p class="text-sm text-fg-muted">{t("playit.ownershipNotVerified")}</p>
            {!authenticated && (
              <Button
                variant="ghost"
                class="!px-3 !py-1.5 !text-xs sm:!text-sm"
                disabled={busy}
                onClick={onOpenAccount}
              >
                {t("playit.signIn")}
              </Button>
            )}
          </div>
        )}
        {ownership?.ownership === "no_agent" && (
          <p class="text-sm text-fg-muted">{t("playit.ownershipNoAgent")}</p>
        )}
      </div>
    </div>
  );
}

function AgentActions({
  lifecycle,
  authenticated,
  busy,
  onConnectDirect,
  onDisconnectAgent,
  onReconnectAgent,
  onOpenAccount,
}: {
  lifecycle: PlayitStatus["status"] | undefined;
  authenticated: boolean;
  busy: boolean;
  onConnectDirect: () => void;
  onDisconnectAgent: () => void;
  onReconnectAgent: () => void;
  onOpenAccount: () => void;
}) {
  const t = useT();

  // Only the actions valid for the current lifecycle render: reconnect
  // while connected (or change while nothing exists) would be noise.
  let actions: ComponentChildren = null;
  switch (lifecycle) {
    case "connected":
      actions = (
        <Button variant="ghost" disabled={busy} onClick={onDisconnectAgent}>
          {t("playit.disconnectAgent")}
        </Button>
      );
      break;
    case "reconnecting":
    case "unavailable":
    case "unsupported":
    case "error":
      actions = (
        <>
          <Button variant="ghost" disabled={busy} onClick={onReconnectAgent}>
            {t("playit.reconnectAgent")}
          </Button>
          <Button variant="ghost" disabled={busy} onClick={onDisconnectAgent}>
            {t("playit.disconnectAgent")}
          </Button>
        </>
      );
      break;
    case "needs_claim":
      actions = authenticated ? (
        <Button variant="primary" disabled={busy} onClick={onConnectDirect}>
          {busy ? t("playit.startingClaim") : t("playit.connectDevice")}
        </Button>
      ) : (
        <Button variant="primary" disabled={busy} onClick={onOpenAccount}>
          {t("playit.signIn")}
        </Button>
      );
      break;
    default:
      actions = null;
  }

  if (!actions) return null;
  return (
    <div class="flex flex-wrap items-center justify-end gap-2">{actions}</div>
  );
}
