import { useState } from "preact/hooks";
import { Banner, Button, Field, Input } from "../ui";
import { Modal } from "../Modal";
import { useT } from "../../i18n";
import { errorText } from "./helpers";

export type ChangeAccountInput = {
  email: string;
  password: string;
  name?: string;
  acknowledge: boolean;
};

/**
 * Switch to a different Playit account. A 409 from the backend means the
 * old account owns this agent, so the modal offers the explicit
 * acknowledgement instead of failing silently.
 */
export function ChangeAccountModal({
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
