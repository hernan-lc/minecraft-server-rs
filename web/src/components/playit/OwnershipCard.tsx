import { useState } from "preact/hooks";
import { Banner, Button, Card, Field, Input } from "../ui";
import * as Icon from "../icons";
import { Modal } from "../Modal";
import { useT } from "../../i18n";
import type { AgentOwnershipInfo, PlayitAuthSession } from "../../types";
import { errorText } from "./helpers";

export type ChangeAccountInput = {
  email: string;
  password: string;
  name?: string;
  acknowledge: boolean;
};

/**
 * Agent lifecycle and account/agent ownership, independent of the account
 * login card. The agent runs on its own secret, so disconnect/reconnect
 * stay available while signed out.
 */
export function OwnershipCard({
  agentId,
  needsClaim,
  authSession,
  ownership,
  ownershipError,
  busy,
  onConnectDirect,
  onReconnectAgent,
  onDisconnectAgent,
  onChangeAccount,
}: {
  agentId: string | null;
  needsClaim: boolean;
  authSession: PlayitAuthSession | null;
  ownership: AgentOwnershipInfo | null;
  ownershipError: string | null;
  busy: boolean;
  onConnectDirect: () => void;
  onReconnectAgent: () => void;
  onDisconnectAgent: () => void;
  onChangeAccount: (input: ChangeAccountInput) => Promise<void>;
}) {
  const t = useT();
  const [changeOpen, setChangeOpen] = useState(false);
  const authenticated = authSession?.authenticated === true;
  const foreignAccount = ownership?.ownership === "different_account";

  return (
    <Card
      title={t("playit.agentSection")}
      actions={
        <Button
          variant="ghost"
          icon={<Icon.User size={15} />}
          class="shrink-0 !px-3 !py-1.5 !text-xs sm:!text-sm"
          disabled={busy}
          onClick={() => setChangeOpen(true)}
        >
          {t("playit.changeAccount")}
        </Button>
      }
    >
      <p class="mb-3 text-sm text-fg-muted">{t("playit.agentControlsExplain")}</p>

      <dl class="grid grid-cols-1 gap-2 sm:grid-cols-2 sm:gap-4">
        <div class="min-w-0 rounded-xl border border-ink-700 bg-ink-900/60 px-3 py-2.5 sm:rounded-lg sm:px-4 sm:py-3">
          <dt class="truncate text-[10px] uppercase tracking-wider text-fg-muted sm:text-xs">
            {t("playit.agentId")}
          </dt>
          <dd class="mt-1 truncate font-mono text-sm text-fg sm:text-base">
            {agentId ?? t("playit.noAgentId")}
          </dd>
        </div>
        <div class="min-w-0 rounded-xl border border-ink-700 bg-ink-900/60 px-3 py-2.5 sm:rounded-lg sm:px-4 sm:py-3">
          <dt class="truncate text-[10px] uppercase tracking-wider text-fg-muted sm:text-xs">
            {t("playit.ownershipSection")}
          </dt>
          <dd class="mt-1 truncate text-sm font-medium text-fg sm:text-base">
            {ownership
              ? t(`playit.ownershipStates.${ownership.ownership}` as "playit.ownershipStates.unknown")
              : (ownershipError ?? t("common.loading"))}
          </dd>
        </div>
      </dl>

      {ownership && !foreignAccount && ownership.ownership === "matched" && (
        <p class="mt-3 text-sm text-fg-muted">{t("playit.ownershipMatchedExplain")}</p>
      )}
      {ownership && ownership.ownership === "unknown" && (
        <p class="mt-3 text-sm text-fg-muted">{t("playit.ownershipUnknownExplain")}</p>
      )}
      {ownership && ownership.ownership === "no_agent" && (
        <p class="mt-3 text-sm text-fg-muted">{t("playit.ownershipNoAgentExplain")}</p>
      )}
      {foreignAccount && (
        <div class="mt-3">
          <Banner kind="error">{t("playit.foreignAccountWarning")}</Banner>
        </div>
      )}
      {ownershipError && !ownership && (
        <div class="mt-3">
          <Banner kind="error">{ownershipError}</Banner>
        </div>
      )}

      <div class="mt-4 flex flex-wrap items-center gap-2">
        {needsClaim && authenticated && (
          <Button variant="primary" disabled={busy} onClick={onConnectDirect}>
            {busy ? t("playit.startingClaim") : t("playit.connectAccount")}
          </Button>
        )}
        <Button variant="ghost" disabled={busy} onClick={onReconnectAgent}>
          {t("playit.reconnectAgent")}
        </Button>
        <Button variant="ghost" disabled={busy} onClick={onDisconnectAgent}>
          {t("playit.disconnectAgent")}
        </Button>
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

function ChangeAccountModal({
  onClose,
  onChangeAccount,
}: {
  onClose: () => void;
  onChangeAccount: (input: ChangeAccountInput) => Promise<void>;
}) {
  const t = useT();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [name, setName] = useState("");
  const [changing, setChanging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [needsAck, setNeedsAck] = useState(false);

  async function submit(acknowledge: boolean, event?: Event) {
    event?.preventDefault();
    if (changing) return;
    setChanging(true);
    setError(null);
    try {
      const trimmedName = name.trim();
      await onChangeAccount({
        email,
        password,
        ...(trimmedName ? { name: trimmedName } : {}),
        acknowledge,
      });
      onClose();
    } catch (caught) {
      // A 409 means the old account owns this agent: offer the explicit
      // acknowledgement instead of failing silently.
      const status =
        typeof caught === "object" && caught !== null && "status" in caught
          ? (caught as { status?: unknown }).status
          : undefined;
      if (status === 409) {
        setNeedsAck(true);
        setError(errorText(caught, t("playit.changeAccountWarning")));
      } else {
        setError(errorText(caught, t("errors.playitAction")));
      }
    } finally {
      setChanging(false);
    }
  }

  return (
    <Modal
      title={t("playit.changeAccountTitle")}
      onClose={onClose}
      width="sm"
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={changing}>
            {t("common.cancel")}
          </Button>
          {needsAck ? (
            <Button
              variant="primary"
              disabled={changing}
              onClick={() => void submit(true)}
            >
              {changing ? t("playit.switchingAccount") : t("playit.changeAccount")}
            </Button>
          ) : (
            <Button
              type="submit"
              form="playit-change-account-form"
              variant="primary"
              disabled={changing}
            >
              {changing ? t("playit.switchingAccount") : t("playit.changeAccount")}
            </Button>
          )}
        </>
      }
    >
      <form
        id="playit-change-account-form"
        onSubmit={(event) => void submit(false, event)}
        class="space-y-4"
      >
        <p class="text-sm text-fg-muted">{t("playit.changeAccountExplain")}</p>
        {error && <Banner kind={needsAck ? "info" : "error"}>{error}</Banner>}
        {needsAck && (
          <p class="text-sm text-fg-muted">{t("playit.changeAccountWarning")}</p>
        )}
        <Field label={t("playit.email")}>
          <Input
            type="email"
            autoComplete="username"
            value={email}
            onInput={(event) => setEmail(event.currentTarget.value)}
          />
        </Field>
        <Field label={t("playit.password")}>
          <Input
            type="password"
            autoComplete="current-password"
            value={password}
            onInput={(event) => setPassword(event.currentTarget.value)}
          />
        </Field>
        <Field label={t("playit.agentNameOptional")}>
          <Input
            value={name}
            maxLength={64}
            onInput={(event) => setName(event.currentTarget.value)}
          />
        </Field>
      </form>
    </Modal>
  );
}
