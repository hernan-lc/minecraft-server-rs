import type { Page } from "playwright";
import { describe, expect, it, vi } from "vitest";
import { installNetworkDiagnostics, MAX_NETWORK_EVENTS, safeUrl } from "./networkDiagnostics.js";

describe("demo network diagnostics", () => {
  it("removes tickets and other query material from diagnostic URLs", () => {
    const sanitized = safeUrl(
      "ws://127.0.0.1:8080/api/servers/srv-1/ws?ticket=super-secret&x=1",
    );

    expect(sanitized).toBe("ws://127.0.0.1:8080/api/servers/srv-1/ws");
    expect(sanitized).not.toContain("super-secret");
    expect(sanitized).not.toContain("ticket");
  });

  it("keeps only the safe path for malformed URLs", () => {
    expect(safeUrl("/api/servers/srv-1/logs?token=secret")).toBe("/api/servers/srv-1/logs");
  });

  it("records backfill milestones and bounds detailed history", () => {
    const listeners = new Map<string, (value: any) => void>();
    const page = {
      on: (event: string, listener: (value: any) => void) => {
        listeners.set(event, listener);
        return page;
      },
    } as unknown as Page;
    const log = vi.spyOn(console, "log").mockImplementation(() => {});

    try {
      const diagnostics = installNetworkDiagnostics(page);
      const socketListeners = new Map<string, (value: any) => void>();
      const socket = {
        url: () => "ws://127.0.0.1:8080/api/servers/srv-1/ws?ticket=secret",
        on: (event: string, listener: (value: any) => void) => {
          socketListeners.set(event, listener);
          return socket;
        },
      };
      listeners.get("websocket")?.(socket);
      socketListeners.get("framereceived")?.({
        payload: JSON.stringify({ type: "backfill", lines: [] }),
      });

      const snapshot = diagnostics.snapshot();
      expect(snapshot.events.map((event) => event.event)).toEqual(
        expect.arrayContaining(["ws-open", "backfill-received", "usable-console"]),
      );
      expect(snapshot.events.some((event) => event.url?.includes("ticket=secret"))).toBe(false);

      for (let index = 0; index < MAX_NETWORK_EVENTS + 5; index += 1) {
        diagnostics.mark("retry-scheduled", `retry ${index}`);
      }
      expect(diagnostics.snapshot().events).toHaveLength(MAX_NETWORK_EVENTS);
    } finally {
      log.mockRestore();
    }
  });
});
