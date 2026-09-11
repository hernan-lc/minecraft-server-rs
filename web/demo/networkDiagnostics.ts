import type { Page, WebSocket as PlaywrightWebSocket } from "playwright";

export interface NetworkDiagnosticEvent {
  at: number;
  kind: "http" | "requestfailed" | "websocket";
  event: string;
  method?: string;
  url?: string;
  status?: number;
  detail?: string;
}

export interface NetworkDiagnosticsSnapshot {
  events: NetworkDiagnosticEvent[];
  http: {
    responses: number;
    ticketResponses: number;
    lastTicketStatus: number | null;
  };
  websockets: {
    created: number;
    connected: number;
    framesReceived: number;
    framesSent: number;
    errors: number;
    closed: number;
  };
}

export type NetworkDiagnosticMilestone =
  | "ws-open"
  | "backfill-received"
  | "usable-console"
  | "backfill-timeout"
  | "retry-scheduled";

export interface NetworkDiagnostics {
  snapshot(): NetworkDiagnosticsSnapshot;
  mark(event: NetworkDiagnosticMilestone, detail?: string): void;
}

export const MAX_NETWORK_EVENTS = 1_000;

/** Remove query strings and fragments before a URL can reach terminal logs. */
export function safeUrl(raw: string): string {
  try {
    const url = new URL(raw);
    return `${url.origin}${url.pathname}`;
  } catch {
    return raw.split(/[?#]/, 1)[0] || "<invalid-url>";
  }
}

function safeDetail(value: unknown): string {
  return String(value)
    .replace(
      /((?:password|authorization|cookie|ticket|csrf|token|secret)\s*[:=]\s*)([^\s,;}]+)/gi,
      "$1<redacted>",
    )
    .slice(0, 500);
}

function relevantPath(raw: string): string | null {
  try {
    const url = new URL(raw);
    return url.pathname.startsWith("/api/servers/") ? url.pathname : null;
  } catch {
    return null;
  }
}

/**
 * Observe the panel's server API and console WebSocket without routing or
 * rewriting traffic. This stays in the demo process and never exposes the
 * one-use ticket or browser credentials in logs.
 */
export function installNetworkDiagnostics(page: Page): NetworkDiagnostics {
  const startedAt = Date.now();
  const events: NetworkDiagnosticEvent[] = [];
  const sockets = new Map<
    PlaywrightWebSocket,
    { url: string; opened: boolean; backfillReceived: boolean }
  >();
  let responses = 0;
  let ticketResponses = 0;
  let lastTicketStatus: number | null = null;
  let created = 0;
  let connected = 0;
  let framesReceived = 0;
  let framesSent = 0;
  let errors = 0;
  let closed = 0;

  const record = (event: NetworkDiagnosticEvent): void => {
    events.push({ ...event, detail: event.detail ? safeDetail(event.detail) : undefined });
    if (events.length > MAX_NETWORK_EVENTS) {
      events.splice(0, events.length - MAX_NETWORK_EVENTS);
    }
  };

  const mark = (event: NetworkDiagnosticMilestone, detail?: string): void => {
    record({
      at: (Date.now() - startedAt) / 1000,
      kind: "websocket",
      event,
      detail,
    });
    console.log(`[demo/ws] ${event}${detail ? ` ${safeDetail(detail)}` : ""}`);
  };

  page.on("response", (response) => {
    const path = relevantPath(response.url());
    if (!path) return;
    responses += 1;
    const method = response.request().method();
    const status = response.status();
    const isTicket = path.endsWith("/ws/ticket");
    if (isTicket) {
      ticketResponses += 1;
      lastTicketStatus = status;
    }
    record({
      at: (Date.now() - startedAt) / 1000,
      kind: "http",
      event: "response",
      method,
      url: safeUrl(response.url()),
      status,
    });
    console.log(`[demo/net] ${method} ${path} -> ${status}`);
  });

  page.on("requestfailed", (request) => {
    const path = relevantPath(request.url());
    if (!path) return;
    const detail = request.failure()?.errorText ?? "request failed";
    record({
      at: (Date.now() - startedAt) / 1000,
      kind: "requestfailed",
      event: "failed",
      method: request.method(),
      url: safeUrl(request.url()),
      detail,
    });
    console.error(`[demo/net] failed ${request.method()} ${path} ${safeDetail(detail)}`);
  });

  page.on("websocket", (socket) => {
    const url = safeUrl(socket.url());
    const state = { url, opened: false, backfillReceived: false };
    created += 1;
    sockets.set(socket, state);
    record({
      at: (Date.now() - startedAt) / 1000,
      kind: "websocket",
      event: "created",
      url,
    });
    console.log(`[demo/ws] created ${url}`);
    if (created > 1) mark("retry-scheduled", "new console socket");

    socket.on("framereceived", ({ payload }) => {
      framesReceived += 1;
      if (!state.opened) {
        state.opened = true;
        mark("ws-open", url);
      }
      let messageType: unknown = null;
      try {
        const text = typeof payload === "string" ? payload : payload.toString();
        messageType = (JSON.parse(text) as { type?: unknown }).type;
      } catch {
        // Payloads are intentionally never retained or printed.
      }
      if (messageType === "backfill" && !state.backfillReceived) {
        state.backfillReceived = true;
        connected += 1;
        mark("backfill-received", url);
        mark("usable-console", url);
        record({
          at: (Date.now() - startedAt) / 1000,
          kind: "websocket",
          event: "connected",
          url,
        });
        console.log(`[demo/ws] connected ${url}`);
      }
      record({
        at: (Date.now() - startedAt) / 1000,
        kind: "websocket",
        event: "frame received",
        url,
      });
    });

    socket.on("framesent", () => {
      framesSent += 1;
      record({
        at: (Date.now() - startedAt) / 1000,
        kind: "websocket",
        event: "frame sent",
        url,
      });
    });

    socket.on("socketerror", (error) => {
      errors += 1;
      const detail = safeDetail(error);
      record({
        at: (Date.now() - startedAt) / 1000,
        kind: "websocket",
        event: "error",
        url,
        detail,
      });
      console.error(`[demo/ws] error ${url}: ${detail}`);
    });

    socket.on("close", () => {
      closed += 1;
      if (!state.backfillReceived) {
        mark("backfill-timeout", "socket closed before backfill");
      }
      record({
        at: (Date.now() - startedAt) / 1000,
        kind: "websocket",
        event: "closed",
        url,
      });
      console.log(`[demo/ws] closed ${url}`);
      sockets.delete(socket);
    });
  });

  return {
    mark,
    snapshot: () => ({
      events: events.map((event) => ({ ...event })),
      http: { responses, ticketResponses, lastTicketStatus },
      websockets: {
        created,
        connected,
        framesReceived,
        framesSent,
        errors,
        closed,
      },
    }),
  };
}
