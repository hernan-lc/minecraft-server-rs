import { useState } from "preact/hooks";
import { Banner, Button, Card, Empty, Field, Select } from "../ui";
import { Modal } from "../Modal";
import { useT } from "../../i18n";
import type { PlayitAgent } from "../../types";
import type { AgentDraft } from "./helpers";

export function AgentsCard({
  agents,
  agentsFailure,
  currentAgentId,
  busy,
  onDeleteAgent,
}: {
  agents: PlayitAgent[];
  agentsFailure: string | null;
  currentAgentId: string | null;
  busy: boolean;
  onDeleteAgent: (agent: PlayitAgent, draft: AgentDraft) => void;
}) {
  const t = useT();
  const [pending, setPending] = useState<PlayitAgent | null>(null);

  return (
    <Card title={t("playit.agentsSection")}>
      {agentsFailure ? (
        <Banner kind="error">{agentsFailure}</Banner>
      ) : agents.length === 0 ? (
        <Empty>{t("playit.noAgents")}</Empty>
      ) : (
        <ul class="divide-y divide-ink-700">
          {agents.map((agent) => (
            <AgentRow
              key={agent.id}
              agent={agent}
              isCurrent={agent.id === currentAgentId}
              busy={busy}
              onDelete={() => setPending(agent)}
            />
          ))}
        </ul>
      )}

      {pending && (
        <DeleteAgentModal
          agent={pending}
          agents={agents}
          busy={busy}
          onClose={() => setPending(null)}
          onConfirm={(draft) => {
            onDeleteAgent(pending, draft);
            setPending(null);
          }}
        />
      )}
    </Card>
  );
}

function AgentRow({
  agent,
  isCurrent,
  busy,
  onDelete,
}: {
  agent: PlayitAgent;
  isCurrent: boolean;
  busy: boolean;
  onDelete: () => void;
}) {
  const t = useT();
  return (
    <li class="flex items-center gap-3 py-3 first:pt-0 last:pb-0">
      <div class="min-w-0 flex-1">
        <p class="truncate text-sm font-medium">
          {agent.name}
          {isCurrent && (
            <span class="ml-2 text-xs font-normal text-accent">{t("playit.currentAgent")}</span>
          )}
        </p>
        <p class="truncate font-mono text-xs text-fg-muted">{agent.id}</p>
      </div>
      {!isCurrent && (
        <Button
          variant="ghost"
          class="shrink-0 !px-3 !py-1.5 !text-xs"
          disabled={busy}
          onClick={onDelete}
        >
          {t("playit.deleteAgent")}
        </Button>
      )}
    </li>
  );
}

function DeleteAgentModal({
  agent,
  agents,
  busy,
  onClose,
  onConfirm,
}: {
  agent: PlayitAgent;
  agents: PlayitAgent[];
  busy: boolean;
  onClose: () => void;
  onConfirm: (draft: AgentDraft) => void;
}) {
  const t = useT();
  const [moveTo, setMoveTo] = useState("");
  const [disable, setDisable] = useState(false);

  function submit(event?: Event) {
    event?.preventDefault();
    onConfirm({ moveTo, disable });
  }

  return (
    <Modal
      title={t("playit.deleteAgentTitle", { name: agent.name })}
      onClose={onClose}
      width="sm"
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button variant="danger" disabled={busy} onClick={() => void submit()}>
            {t("playit.deleteAgent")}
          </Button>
        </>
      }
    >
      <form onSubmit={submit} class="space-y-4">
        <p class="text-sm text-fg-muted">{t("playit.deleteAgentUnassignBody")}</p>
        <Field label={t("playit.moveTunnelsTo")}>
          <Select
            aria-label={t("playit.moveTunnelsTo")}
            value={moveTo}
            onInput={(event) => setMoveTo(event.currentTarget.value)}
          >
            <option value="">{t("playit.unassignTunnels")}</option>
            {agents
              .filter((other) => other.id !== agent.id)
              .map((other) => (
                <option key={other.id} value={other.id}>
                  {other.name}
                </option>
              ))}
          </Select>
        </Field>
        <label class="flex items-center gap-2 text-sm text-fg-muted">
          <input
            type="checkbox"
            checked={disable}
            onChange={(event) => setDisable(event.currentTarget.checked)}
            class="size-4 rounded border-ink-600 bg-ink-900 accent-[var(--color-accent)]"
          />
          {t("playit.disableTunnels")}
        </label>
      </form>
    </Modal>
  );
}
