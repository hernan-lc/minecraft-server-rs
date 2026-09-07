import { Actions, Banner, Button, Card } from "../ui";
import { useT } from "../../i18n";
import type { PlayitAccount, PlayitStatus } from "../../types";
import { accountLabel, safeExternalUrl, stateLabel, statusTone } from "./helpers";

export function ConnectionCard({
  status,
  account,
  accountError,
  claimUrl,
  busy,
  onClaim,
}: {
  status: PlayitStatus | null;
  account: PlayitAccount | null;
  accountError: string | null;
  claimUrl: string | null;
  busy: boolean;
  onClaim: () => void;
}) {
  const t = useT();
  const loginUrl = safeExternalUrl(account?.login_link);
  const claimingComplete =
    status?.status === "connected" || account?.status === "verified";
  const activeClaimUrl = claimingComplete ? null : (claimUrl ?? account?.claim_url);
  const safeClaimUrl = safeExternalUrl(activeClaimUrl);

  return (
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
          <Button variant="primary" class="w-full sm:w-auto" disabled={busy} onClick={onClaim}>
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
            {t("playit.claimInstructions")}{" "}
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
      <p class={`mt-1 truncate text-sm font-medium sm:text-base ${tone ? colours[tone] : "text-fg"}`}>
        {value}
      </p>
    </div>
  );
}
