import { describe, expect, it } from "vitest";
import { safeUrl } from "./networkDiagnostics.js";

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
});
