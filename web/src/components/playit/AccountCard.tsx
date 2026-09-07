import { useEffect, useState } from "preact/hooks";
import { Banner, Button, Card, Field, Input } from "../ui";
import * as Icon from "../icons";
import { Modal } from "../Modal";
import { useT } from "../../i18n";
import type { PlayitAuthSession } from "../../types";

/**
 * Minimalist account card: a single status row with one primary action.
 * The email/password and TOTP inputs live in modals so the page stays
 * scannable when signed out.
 */
export function AccountCard({
  authFailure,
  authSession,
  authBusy,
  busy,
  needsClaim,
  onLogin,
  onTotp,
  onLogout,
  onConnectDirect,
  onReconnectAgent,
  onDisconnectAgent,
}: {
  authFailure: string | null;
  authSession: PlayitAuthSession | null;
  authBusy: boolean;
  busy: boolean;
  needsClaim: boolean;
  onLogin: (email: string, password: string) => void;
  onTotp: (code: string) => void;
  onLogout: () => void;
  onConnectDirect: () => void;
  onReconnectAgent: () => void;
  onDisconnectAgent: () => void;
}) {
  const t = useT();
  const [signInOpen, setSignInOpen] = useState(false);
  const [totpOpen, setTotpOpen] = useState(false);

  const authenticated = authSession?.authenticated === true;
  const requiresTotp = authSession?.requires_totp === true;

  // Keep the dialogs in sync with the session: a successful login closes
  // the sign-in modal, a TOTP challenge opens verification, and a verified
  // session closes everything.
  useEffect(() => {
    if (authenticated) setSignInOpen(false);
    if (authenticated) setTotpOpen(false);
    else if (requiresTotp) {
      setSignInOpen(false);
      setTotpOpen(true);
    }
  }, [authenticated, requiresTotp]);

  return (
    <Card
      title={t("playit.accountSection")}
      actions={
        !authenticated && !requiresTotp ? (
          <Button
            variant="primary"
            icon={<Icon.User size={15} />}
            class="shrink-0 !px-3 !py-1.5 !text-xs sm:!text-sm"
            onClick={() => setSignInOpen(true)}
          >
            {t("playit.signIn")}
          </Button>
        ) : requiresTotp ? (
          <Button
            variant="primary"
            class="shrink-0 !px-3 !py-1.5 !text-xs sm:!text-sm"
            onClick={() => setTotpOpen(true)}
          >
            {t("playit.verify")}
          </Button>
        ) : (
          <Button
            variant="ghost"
            class="shrink-0 !px-3 !py-1.5 !text-xs sm:!text-sm"
            disabled={authBusy}
            onClick={onLogout}
          >
            {t("playit.signOut")}
          </Button>
        )
      }
    >
      {authFailure && !signInOpen && !totpOpen && (
        <div class="mb-3">
          <Banner kind="error">{authFailure}</Banner>
        </div>
      )}

      {!authenticated && !requiresTotp && (
        <p class="text-sm text-fg-muted">{t("playit.signInExplain")}</p>
      )}

      {requiresTotp && !totpOpen && (
        <p class="text-sm text-fg-muted">{t("playit.totpPrompt")}</p>
      )}

      {authenticated && (
        <div class="flex flex-col gap-3">
          <p class="truncate text-sm text-fg-muted">
            {t("playit.signedInAs", {
              id: authSession?.account_id ?? t("common.unknown"),
              status: authSession?.account_status ?? t("common.unknown"),
            })}
            {authSession?.read_only ? ` · ${t("playit.readOnly")}` : ""}
          </p>
          <div class="flex flex-wrap items-center gap-2">
            {needsClaim && (
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
        </div>
      )}

      {signInOpen && (
        <SignInModal
          authFailure={authFailure}
          authBusy={authBusy}
          onClose={() => setSignInOpen(false)}
          onLogin={onLogin}
        />
      )}
      {totpOpen && (
        <TotpModal
          authFailure={authFailure}
          authBusy={authBusy}
          onClose={() => setTotpOpen(false)}
          onLogout={onLogout}
          onVerify={onTotp}
        />
      )}
    </Card>
  );
}

function SignInModal({
  authFailure,
  authBusy,
  onClose,
  onLogin,
}: {
  authFailure: string | null;
  authBusy: boolean;
  onClose: () => void;
  onLogin: (email: string, password: string) => void;
}) {
  const t = useT();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");

  function submit(event?: Event) {
    event?.preventDefault();
    if (authBusy) return;
    onLogin(email, password);
  }

  return (
    <Modal
      title={t("playit.signInTitle")}
      onClose={onClose}
      width="sm"
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={authBusy}>
            {t("common.cancel")}
          </Button>
          <Button
            type="submit"
            form="playit-signin-form"
            variant="primary"
            disabled={authBusy}
          >
            {authBusy ? t("playit.signingIn") : t("playit.signIn")}
          </Button>
        </>
      }
    >
      <form id="playit-signin-form" onSubmit={submit} class="space-y-4">
        <p class="text-sm text-fg-muted">{t("playit.signInExplain")}</p>
        {authFailure && <Banner kind="error">{authFailure}</Banner>}
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
      </form>
    </Modal>
  );
}

function TotpModal({
  authFailure,
  authBusy,
  onClose,
  onLogout,
  onVerify,
}: {
  authFailure: string | null;
  authBusy: boolean;
  onClose: () => void;
  onLogout: () => void;
  onVerify: (code: string) => void;
}) {
  const t = useT();
  const [code, setCode] = useState("");

  function submit(event?: Event) {
    event?.preventDefault();
    if (authBusy) return;
    onVerify(code);
  }

  return (
    <Modal
      title={t("playit.totpTitle")}
      onClose={onClose}
      width="sm"
      footer={
        <>
          <Button
            variant="ghost"
            disabled={authBusy}
            onClick={() => {
              onLogout();
              onClose();
            }}
          >
            {t("common.cancel")}
          </Button>
          <Button type="submit" form="playit-totp-form" variant="primary" disabled={authBusy}>
            {t("playit.verify")}
          </Button>
        </>
      }
    >
      <form id="playit-totp-form" onSubmit={submit} class="space-y-4">
        <p class="text-sm text-fg-muted">{t("playit.totpPrompt")}</p>
        {authFailure && <Banner kind="error">{authFailure}</Banner>}
        <Field label={t("playit.totpCode")}>
          <Input
            inputMode="numeric"
            autoComplete="one-time-code"
            value={code}
            onInput={(event) => setCode(event.currentTarget.value)}
          />
        </Field>
      </form>
    </Modal>
  );
}
