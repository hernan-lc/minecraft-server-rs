import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { ApiError, api, openConsole } from "../api";
import { useT } from "../i18n";
import * as Icon from "./icons";
import { IconButton } from "./ui";
import type { ConsoleLine, ProgressState, ServerEvent, Status } from "../types";

/** Keep the DOM bounded; the backend keeps the authoritative buffer. */
const MAX_LINES = 2000;

const MAX_RETRY_DELAY_MS = 30_000;
export const INITIAL_BACKFILL_TIMEOUT_MS = 10_000;

export type ConsoleConnectionState =
  | "connecting"
  | "connected"
  | "reconnecting"
  | "disconnected";

export type ConsoleConnectionErrorKind =
  | "ticket"
  | "handshake"
  | "network"
  | "authorization"
  | "server";

/** Exponential retry with a bounded tail: 1s, 2s, 4s, …, 30s, 30s. */
export function consoleRetryDelay(attempt: number): number {
  return Math.min(1000 * 2 ** Math.max(0, attempt), MAX_RETRY_DELAY_MS);
}

export function consoleErrorKind(error: unknown): ConsoleConnectionErrorKind {
  if (error instanceof ApiError) {
    if (error.status === 401) return "authorization";
    if (error.status === 403 || error.status === 404) return "authorization";
    if (error.status >= 500 || error.status === 429) return "server";
    return "ticket";
  }
  return "network";
}

export function isTerminalConsoleError(error: unknown): boolean {
  return error instanceof ApiError && [401, 403, 404].includes(error.status);
}

type ConsoleEntry =
  | { kind: "line"; line: ConsoleLine }
  | { kind: "notice"; id: number; line: ConsoleLine };

function uniqueLines(lines: ConsoleLine[]): ConsoleLine[] {
  const seen = new Set<number>();
  return lines.filter((line) => {
    if (seen.has(line.seq)) return false;
    seen.add(line.seq);
    return true;
  });
}

function isUsableBackfill(
  message: ServerEvent,
): message is Extract<ServerEvent, { type: "backfill" }> {
  return (
    message.type === "backfill" &&
    Array.isArray(message.lines) &&
    typeof message.status === "object" &&
    message.status !== null &&
    typeof message.status.status === "string"
  );
}

/** Merge a reconnect backfill with lines received while the socket reopened. */
export function mergeBackfill(previous: ConsoleEntry[], incoming: ConsoleLine[]): ConsoleEntry[] {
  const bySeq = new Map<number, ConsoleLine>();
  for (const entry of previous) {
    if (entry.kind === "line") bySeq.set(entry.line.seq, entry.line);
  }
  for (const line of incoming) bySeq.set(line.seq, line);

  return [...bySeq.values()]
    .sort((left, right) => left.seq - right.seq)
    .slice(-MAX_LINES)
    .map((line) => ({ kind: "line" as const, line }));
}

/** Colour a line by what it obviously is, without parsing log formats strictly. */
function lineClass(line: ConsoleLine): string {
  if (line.stream === "system") return "text-sky-400";
  if (line.stream === "stderr") return "text-red-400";
  if (/\bERROR\b|\bSEVERE\b|Exception|\bFATAL\b/.test(line.line)) return "text-red-400";
  if (/\bWARN\b/.test(line.line)) return "text-amber-300";
  if (/\bDone \(/.test(line.line)) return "text-accent";
  return "text-fg/85";
}

export function Console({
  serverId,
  status,
  progress,
  onStatus,
  onProgress,
}: {
  serverId: string;
  status?: Status;
  progress?: ProgressState | null;
  onStatus: (status: Status) => void;
  onProgress?: (p: ProgressState | null) => void;
}) {
  const t = useT();
  const [lines, setLines] = useState<ConsoleEntry[]>([]);
  const [connectionState, setConnectionState] = useState<ConsoleConnectionState>("connecting");
  const [lastConnectionError, setLastConnectionError] = useState<ConsoleConnectionErrorKind | null>(null);
  const [draft, setDraft] = useState("");
  const [history, setHistory] = useState<string[]>([]);
  const [historyAt, setHistoryAt] = useState(-1);

  const [expanded, setExpanded] = useState(false);
  const [atBottom, setAtBottom] = useState(true);

  const socket = useRef<WebSocket | null>(null);
  const scroller = useRef<HTMLDivElement | null>(null);
  const pinned = useRef(true);
  const lastSeqRef = useRef<number | null>(null);
  const noticeIdRef = useRef(0);
  const compactProgressPercent =
    status === "preparing" &&
    typeof progress?.fraction === "number" &&
    Number.isFinite(progress.fraction)
      ? Math.round(Math.max(0, Math.min(1, progress.fraction)) * 100)
      : null;

  useEffect(() => {
    setConnectionState("connecting");
    setLastConnectionError(null);
    setLines([]);
    lastSeqRef.current = null;
    noticeIdRef.current = 0;
    let closed = false;
    let retry: number | undefined;
    let backfillTimer: number | undefined;
    let attempt = 0;

    const clearBackfillWatchdog = () => {
      if (backfillTimer !== undefined) {
        window.clearTimeout(backfillTimer);
        backfillTimer = undefined;
      }
    };

    const armBackfillWatchdog = (ws: WebSocket) => {
      clearBackfillWatchdog();
      backfillTimer = window.setTimeout(() => {
        if (closed || socket.current !== ws) return;
        // A browser WebSocket can remain CONNECTING when the upgrade is
        // blocked, and a fast server can deliver the first frame before a
        // consumer observes it. Either way, do not leave the console stuck
        // forever with no close event to trigger the retry path.
        setLastConnectionError("handshake");
        setConnectionState("reconnecting");
        ws.close();
      }, INITIAL_BACKFILL_TIMEOUT_MS);
    };

    const scheduleReconnect = () => {
      if (closed || retry !== undefined) return;
      retry = window.setTimeout(() => {
        retry = undefined;
        void connect();
      }, consoleRetryDelay(attempt));
      attempt += 1;
    };

    const connect = async () => {
      if (closed) return;
      retry = undefined;
      let ws: WebSocket;
      try {
        ws = await openConsole(serverId);
      } catch (error) {
        if (closed) return;
        const kind = consoleErrorKind(error);
        setLastConnectionError(kind);
        if (isTerminalConsoleError(error)) {
          setConnectionState("disconnected");
          return;
        }
        setConnectionState("reconnecting");
        scheduleReconnect();
        return;
      }
      if (closed) {
        ws.close();
        return;
      }
      socket.current = ws;
      // Start this before onopen as well as from onopen. This covers a
      // handshake that never reaches onopen and the small window in which a
      // very fast socket opens before the awaited helper resumes.
      armBackfillWatchdog(ws);

      ws.onopen = () => {
        if (closed) return;
        setConnectionState(attempt === 0 ? "connecting" : "reconnecting");
        armBackfillWatchdog(ws);
      };
      ws.onerror = () => {
        if (closed) return;
        setConnectionState("reconnecting");
        setLastConnectionError("network");
      };
      ws.onclose = (event) => {
        clearBackfillWatchdog();
        if (socket.current !== ws) return;
        socket.current = null;
        if (closed) return;
        // A policy/protocol close is the browser-visible equivalent of a
        // 403/404 handshake failure. Do not keep issuing one-use tickets for
        // a session that the backend has rejected.
        if (event?.code === 1003 || event?.code === 1008) {
          setConnectionState("disconnected");
          setLastConnectionError("authorization");
          return;
        }
        setConnectionState("reconnecting");
        setLastConnectionError("network");
        scheduleReconnect();
      };
      ws.onmessage = (event) => {
        let message: ServerEvent;
        try {
          message = JSON.parse(event.data) as ServerEvent;
        } catch {
          setLastConnectionError("handshake");
          return;
        }
        switch (message.type) {
          case "backfill": {
            if (!isUsableBackfill(message)) {
              setLastConnectionError("handshake");
              return;
            }
            clearBackfillWatchdog();
            attempt = 0;
            setConnectionState("connected");
            setLastConnectionError(null);
            const backfill = uniqueLines(message.lines).slice(-MAX_LINES);
            setLines((previous) => mergeBackfill(previous, backfill));

            const highestLine = backfill.reduce(
              (highest, line) => Math.max(highest, line.seq),
              -1,
            );
            const highest = Math.max(message.through_seq ?? -1, highestLine);
            if (highest >= 0) {
              lastSeqRef.current = Math.max(lastSeqRef.current ?? -1, highest);
            }
            onStatus(message.status.status);
            onProgress?.(
              message.status.status === "preparing" ? message.status.progress : null,
            );
            break;
          }
          case "console": {
            if (lastSeqRef.current !== null && message.seq <= lastSeqRef.current) break;
            lastSeqRef.current = message.seq;
            const line = message as unknown as ConsoleLine;
            setLines((prev) => {
              if (prev.some((entry) => entry.kind === "line" && entry.line.seq === line.seq)) {
                return prev;
              }
              const next = [...prev, { kind: "line" as const, line }];
              return next.length > MAX_LINES ? next.slice(-MAX_LINES) : next;
            });
            break;
          }
          case "status":
            onStatus(message.status);
            if (message.status !== "preparing") {
              onProgress?.(null);
            }
            break;
          case "progress": {
            const p = { stage: message.stage, fraction: message.fraction };
            onProgress?.(p);
            break;
          }
          case "lagged":
            setLines((prev) => [
              ...prev,
              {
                kind: "notice" as const,
                id: ++noticeIdRef.current,
                line: {
                  seq: -1,
                  stream: "system" as const,
                  line: t("console.skipped", { count: message.skipped }),
                },
              },
            ].slice(-MAX_LINES));
            break;
        }
      };

      // `openConsole()` constructs the browser socket before returning its
      // promise. If Chromium completed the upgrade in that gap, the handler
      // above may never receive `onopen`; the watchdog will still close and
      // retry it, while this check keeps the visible state accurate.
      if (ws.readyState === WebSocket.OPEN) {
        setConnectionState(attempt === 0 ? "connecting" : "reconnecting");
        armBackfillWatchdog(ws);
      }
    };

    void connect();
    return () => {
      closed = true;
      if (retry) clearTimeout(retry);
      clearBackfillWatchdog();
      socket.current?.close();
    };
  }, [serverId]);

  // Keep retained output visible while the interactive socket is unavailable.
  // This is intentionally read-only: command input remains gated on the
  // validated WebSocket backfill above.
  useEffect(() => {
    if (connectionState === "connected") return;

    let cancelled = false;
    let polling = false;
    const intervalMs = status === "online" ? 5_000 : 2_500;

    const pollLogs = async () => {
      if (cancelled || polling) return;
      polling = true;
      try {
        const recent = await api.logs(serverId);
        if (cancelled) return;
        const lines = uniqueLines(recent).slice(-MAX_LINES);
        if (lines.length === 0) return;
        setLines((previous) => mergeBackfill(previous, lines));
        const highest = lines.reduce(
          (value, line) => Math.max(value, line.seq),
          -1,
        );
        if (highest >= 0) {
          lastSeqRef.current = Math.max(lastSeqRef.current ?? -1, highest);
        }
      } catch {
        // The WebSocket retry and server status polling remain authoritative;
        // a temporary REST failure should not add another visible error.
      } finally {
        polling = false;
      }
    };

    void pollLogs();
    const timer = window.setInterval(() => {
      void pollLogs();
    }, intervalMs);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [connectionState, serverId, status]);

  // Follow the tail, but stop fighting the operator once they scroll up.
  useEffect(() => {
    if (pinned.current && scroller.current) {
      scroller.current.scrollTop = scroller.current.scrollHeight;
    }
  }, [lines]);

  function onScroll() {
    const el = scroller.current;
    if (!el) return;
    pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
    setAtBottom(pinned.current);
  }

  function scrollToBottom() {
    const el = scroller.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    pinned.current = true;
    setAtBottom(true);
  }

  function send(event: Event) {
    event.preventDefault();
    const command = draft.trim();
    if (!command || !socket.current || socket.current.readyState !== WebSocket.OPEN) return;
    socket.current.send(JSON.stringify({ type: "command", command }));
    setHistory((prev) => [command, ...prev.filter((c) => c !== command)].slice(0, 50));
    setHistoryAt(-1);
    setDraft("");
    pinned.current = true;
  }

  function onKeyDown(event: KeyboardEvent) {
    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
    event.preventDefault();
    const next = event.key === "ArrowUp"
      ? Math.min(historyAt + 1, history.length - 1)
      : Math.max(historyAt - 1, -1);
    setHistoryAt(next);
    setDraft(next === -1 ? "" : history[next]);
  }

  const rendered = useMemo(
    () =>
      lines.map((entry) => (
        <div
          key={entry.kind === "line" ? entry.line.seq : `notice-${entry.id}`}
          data-console-line="true"
          class={`whitespace-pre-wrap break-words ${lineClass(entry.line)}`}
        >
          {entry.line.line}
        </div>
      )),
    [lines],
  );

  return (
    <section
      data-testid="server-console"
      class={
        expanded
          ? "fixed inset-0 z-50 flex flex-col bg-ink-900 p-4"
          : "flex min-h-[18rem] flex-1 flex-col gap-2.5 rounded-2xl border border-ink-700 bg-ink-850 p-3 sm:min-h-0 sm:gap-3 sm:p-5"
      }
    >
      <div class="flex items-center justify-between">
        <h2 class="flex items-center gap-2.5 text-lg font-semibold">
          {t("console.title")}
          {expanded && status && (
            <span class="text-sm font-normal text-fg-muted">
              · {t(`status.${status}` as "status.offline")}
              {compactProgressPercent !== null && ` · ${compactProgressPercent}%`}
            </span>
          )}
          <span
            data-testid="server-console-connection"
            data-state={
              connectionState === "connected"
                ? "connected"
                : connectionState === "disconnected"
                  ? "disconnected"
                  : "reconnecting"
            }
            data-last-error-kind={lastConnectionError ?? undefined}
            class={`size-2.5 rounded-full ${
              connectionState === "connected"
                ? "bg-accent"
                : connectionState === "disconnected"
                  ? "bg-red-500"
                  : "animate-pulse bg-amber-400"
            }`}
            role="status"
            aria-label={
              connectionState === "connected"
                ? t("console.connected")
                : connectionState === "disconnected"
                  ? t("console.disconnectedState")
                  : t("console.reconnecting")
            }
          />
        </h2>

        <IconButton
          label={expanded ? t("console.collapse") : t("console.expand")}
          icon={expanded ? <Icon.Collapse size={17} /> : <Icon.Expand size={17} />}
          onClick={() => setExpanded((v) => !v)}
        />
      </div>

      <div class="relative min-h-0 flex-1 overflow-hidden rounded-xl bg-ink-950">
        <div
          ref={scroller}
          data-testid="server-console-lines"
          onScroll={onScroll}
          class="h-full overflow-y-auto px-3 py-2.5 font-mono text-[13px] leading-relaxed sm:px-4 sm:py-3"
        >
          {rendered.length === 0 ? <p class="text-fg-muted">{t("console.empty")}</p> : rendered}
        </div>

        {/* Only offered when it would do something, so it does not sit there
            inviting a click that changes nothing. */}
        {!atBottom && (
          <div class="absolute bottom-3 right-3">
            <IconButton
              label={t("console.toBottom")}
              side="top"
              icon={<Icon.ArrowDown size={17} />}
              onClick={scrollToBottom}
              class="!bg-ink-800 !text-fg shadow-lg hover:!bg-ink-700"
            />
          </div>
        )}
      </div>

      <form
        onSubmit={send}
        class="flex items-center gap-2 rounded-xl border border-ink-700 bg-ink-950 px-3 py-2.5 focus-within:border-accent/60"
      >
        <span class="shrink-0 text-fg-muted">
          <Icon.Terminal size={16} />
        </span>
        <input
          value={draft}
          onInput={(e) => setDraft((e.target as HTMLInputElement).value)}
          onKeyDown={onKeyDown}
          placeholder={connectionState === "connected" ? t("console.placeholder") : t("console.disconnected")}
          disabled={connectionState !== "connected"}
          spellcheck={false}
          autocomplete="off"
          class="flex-1 bg-transparent font-mono text-sm text-fg placeholder:text-fg-muted/60 focus:outline-none disabled:opacity-50"
        />
      </form>
    </section>
  );
}
