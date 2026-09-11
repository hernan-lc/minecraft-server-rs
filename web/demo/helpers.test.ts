import type { Locator, Page } from "playwright";
import { describe, expect, it, vi } from "vitest";
import {
  classifyStartupFailure,
  waitForGone,
  waitForSelectOptions,
  waitForServerLifecycle,
  waitForServerOnline,
} from "./helpers.js";
import type { ServerDiagnostics } from "./helpers.js";

function lifecyclePage(status: string): Page {
  const locators = {
    "server-status": {
      getAttribute: vi.fn(async (name: string) => (name === "data-status" ? status : null)),
    },
    "server-activity": {
      getAttribute: vi.fn(async (name: string) =>
        name === "data-stage" ? "java-download" : name === "data-fraction" ? "0.25" : null,
      ),
    },
    "server-console-lines": {
      evaluate: vi.fn(async () => ["latest console line"]),
    },
  };

  return {
    getByTestId: vi.fn((testId: string) => locators[testId as keyof typeof locators]),
    waitForFunction: vi.fn(async () => ({ jsonValue: async () => ({ status }) })),
  } as unknown as Page;
}

describe("demo lifecycle helpers", () => {
  it.each(["preparing", "starting", "online"]) (
    "accepts %s as an initial Start acknowledgement",
    async (status) => {
      const page = lifecyclePage(status);

      await expect(waitForServerLifecycle(page)).resolves.toBe(status);
      const statuses = (page.waitForFunction as unknown as ReturnType<typeof vi.fn>).mock
        .calls[0][1];
      expect(statuses).toEqual(["preparing", "starting", "online"]);
    },
  );

  it("does not expect the non-existent running lifecycle value", async () => {
    const page = lifecyclePage("starting");

    await waitForServerLifecycle(page);
    const statuses = (page.waitForFunction as unknown as ReturnType<typeof vi.fn>).mock
      .calls[0][1] as string[];
    expect(statuses).not.toContain("running");
  });

  it("waits for online successfully", async () => {
    const page = lifecyclePage("online");

    await expect(waitForServerOnline(page, 1_000)).resolves.toBeUndefined();
  });

  it("fails immediately when the server crashes", async () => {
    const page = lifecyclePage("crashed");
    const started = Date.now();

    await expect(waitForServerOnline(page, 20 * 60_000)).rejects.toThrow(
      /Server crashed during first boot.*Last progress stage: java-download/s,
    );
    expect(Date.now() - started).toBeLessThan(1_000);
  });

  it("uses hidden rather than detached for modal disappearance", async () => {
    const waitFor = vi.fn().mockResolvedValue(undefined);
    const locator = {
      first: () => ({ waitFor }),
    } as unknown as Locator;

    await waitForGone(locator, "modal", 1_000);

    expect(waitFor).toHaveBeenCalledWith({ state: "hidden", timeout: 1_000 });
  });

  it("preserves a catalog evaluation error in the timeout", async () => {
    const page = {
      getByTestId: vi.fn(() => ({
        evaluate: vi.fn().mockRejectedValue(new Error("catalog request failed")),
      })),
      locator: vi.fn(() => ({ allTextContents: vi.fn().mockResolvedValue([]) })),
    } as unknown as Page;

    await expect(waitForSelectOptions(page, "server-version", "Minecraft versions", 0)).rejects.toThrow(
      /Last error: catalog request failed/,
    );
  });

  it("classifies Guardian output with no browser console as a transport failure", () => {
    const diagnostics: ServerDiagnostics = {
      uiStatus: "starting",
      status: "starting",
      backendStatus: "starting",
      serverId: "srv-1",
      pid: 1234,
      uptimeSecs: 30,
      stage: null,
      fraction: null,
      consoleConnection: "disconnected",
      uiConsoleLines: [],
      backendConsoleLines: [{ stream: "stdout", line: "Preparing spawn area" }],
      consoleLines: [],
      serverHttpStatus: 200,
      logsHttpStatus: 200,
      backendError: null,
    };

    expect(classifyStartupFailure(diagnostics)).toMatch(/Console transport failure/);
  });

  it("classifies a Paper Done line that did not promote status", () => {
    const diagnostics: ServerDiagnostics = {
      uiStatus: "starting",
      status: "starting",
      backendStatus: "starting",
      serverId: "srv-1",
      pid: 1234,
      uptimeSecs: 30,
      stage: null,
      fraction: null,
      consoleConnection: "connected",
      uiConsoleLines: ["Done (1.2s)! For help, type \"help\""],
      backendConsoleLines: [{ stream: "stdout", line: "Done (1.2s)! For help, type \"help\"" }],
      consoleLines: [],
      serverHttpStatus: 200,
      logsHttpStatus: 200,
      backendError: null,
    };

    expect(classifyStartupFailure(diagnostics)).toMatch(/readiness detection failure/);
  });

  it("classifies an online backend with a stale UI", () => {
    const diagnostics: ServerDiagnostics = {
      uiStatus: "starting",
      status: "starting",
      backendStatus: "online",
      serverId: "srv-1",
      pid: 1234,
      uptimeSecs: 30,
      stage: null,
      fraction: null,
      consoleConnection: "connected",
      uiConsoleLines: [],
      backendConsoleLines: [],
      consoleLines: [],
      serverHttpStatus: 200,
      logsHttpStatus: 200,
      backendError: null,
    };

    expect(classifyStartupFailure(diagnostics)).toMatch(/Frontend status synchronization failure/);
  });
});
