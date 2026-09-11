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

export interface NetworkDiagnostics {
  snapshot(): NetworkDiagnosticsSnapshot;
}

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
  const sockets = new Map<PlaywrightWebSocket, { url: string; connected: boolean }>();
  let responses = 0;
  let ticketResponses = 0;
  let lastTicketStatus: number | null = null;
  let connected = 0;
  let framesReceived = 0;
  let framesSent = 0;
  let errors = 0;
  let closed = 0;

  const record = (event: NetworkDiagnosticEvent): void => {
    events.push({ ...event, detail: event.detail ? safeDetail(event.detail) : undefined });
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
    const state = { url, connected: false };
    sockets.set(socket, state);
    record({
      at: (Date.now() - startedAt) / 1000,
      kind: "websocket",
      event: "created",
      url,
    });
    console.log(`[demo/ws] created ${url}`);

    socket.on("framereceived", () => {
      framesReceived += 1;
      if (!state.connected) {
        state.connected = true;
        connected += 1;
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
    snapshot: () => ({
      events: events.map((event) => ({ ...event })),
      http: { responses, ticketResponses, lastTicketStatus },
      websockets: {
        created: events.filter((event) => event.kind === "websocket" && event.event === "created")
          .length,
        connected,
        framesReceived,
        framesSent,
        errors,
        closed,
      },
    }),
  };
}
