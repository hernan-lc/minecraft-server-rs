import { useState } from "preact/hooks";
import { Button, Field, Input, Select } from "../ui";
import * as Icon from "../icons";
import { Modal } from "../Modal";
import { useT } from "../../i18n";

export type CreateTunnelInput = {
  port: number;
  protocol: "tcp" | "udp" | "both";
  address: string;
  name?: string;
};

/** Standalone tunnel creation, kept apart from the tunnel list itself. */
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
