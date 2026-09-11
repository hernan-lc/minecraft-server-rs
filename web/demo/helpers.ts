import type { Locator, Page } from "playwright";
import type { DemoConfig } from "./config.js";

/**
 * Centralized pacing and condition-based waits.
 *
 * Artificial delays exist only for presentation pacing — never for
 * synchronization. All synchronization uses condition-based waits below.
 */

export function pause(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export type DemoPace = "short" | "normal" | "long" | "reveal";

const PACING_MS: Record<DemoPace, number> = {
  short: 300,
  normal: 600,
  long: 1000,
  reveal: 1500,
};

/** No-op unless slow/published-demo pacing is enabled. */
export async function demoPause(
  config: DemoConfig,
  pace: DemoPace = "normal",
): Promise<void> {
  if (!config.slow) return;
  await pause(PACING_MS[pace]);
}

/** Wrap a demo phase so failures identify the failing step. */
export async function step<T>(name: string, operation: () => Promise<T>): Promise<T> {
  const started = Date.now();
  console.log(`[demo] ${name}`);
  try {
    const result = await operation();
    console.log(
      `[demo] ${name} done in ${((Date.now() - started) / 1000).toFixed(1)}s`,
    );
    return result;
  } catch (error) {
    throw new Error(
      `Demo failed during "${name}": ${error instanceof Error ? error.message : String(error)}`,
      { cause: error },
    );
  }
}

/** Condition-based wait for application state (not a sleep). */
export async function assertVisible(
  locator: Locator,
  stage: string,
  timeoutMs = 30_000,
): Promise<void> {
  try {
    await locator.first().waitFor({ state: "visible", timeout: timeoutMs });
  } catch (error) {
    throw new Error(
      `${stage} did not appear within ${Math.round(timeoutMs / 1000)} seconds`,
      { cause: error },
    );
  }
}

/** Wait until a locator is no longer visible (removed or hidden modal). */
export async function waitForGone(
  locator: Locator,
  stage: string,
  timeoutMs = 30_000,
): Promise<void> {
  try {
    await locator.first().waitFor({ state: "hidden", timeout: timeoutMs });
  } catch (error) {
    throw new Error(
      `${stage} did not disappear within ${Math.round(timeoutMs / 1000)} seconds`,
      { cause: error },
    );
  }
}

/** Read `document.body` text (for detecting setup-already-completed, etc.). */
export async function readBodyText(page: Page): Promise<string> {
  return await page.evaluate(() => document.body?.innerText ?? "");
}

/**
 * Wait until a `<select>` (by test id) has at least one non-empty option.
 * Version lists populate asynchronously from `/api/catalog/*`.
 */
export async function waitForSelectOptions(
  page: Page,
  testId: string,
  stage: string,
  timeoutMs = 90_000,
): Promise<string[]> {
  const deadline = Date.now() + timeoutMs;
  let lastError: unknown = null;
  for (;;) {
    try {
      const options = await page.getByTestId(testId).evaluate((select: HTMLSelectElement) =>
        Array.from(select.options).map((o) => o.value).filter((v) => v !== ""),
      );
      if (options.length > 0) return options;
    } catch (error) {
      lastError = error;
    }

    // The create form reports catalog failures through the normal toast
    // surface. Preserve that backend/API message instead of reducing it to an
    // empty select and a misleading generic timeout.
    const catalogError = await page
      .locator('[data-testid="toast"][data-tone="error"]')
      .allTextContents()
      .then((messages) => messages.map((message) => message.trim()).filter(Boolean).at(-1))
      .catch(() => undefined);
    if (catalogError) lastError = new Error(catalogError);

    if (Date.now() >= deadline) {
      throw new Error(
        `${stage} did not populate within ${Math.round(timeoutMs / 1000)} seconds.${
          lastError ? ` Last error: ${formatError(lastError)}` : ""
        }`,
        { cause: lastError ?? undefined },
      );
    }
    await pause(250);
  }
}

/** Wait until a select's controlled value reaches the expected option. */
export async function waitForSelectValue(
  page: Page,
  testId: string,
  expected: string,
  stage: string,
  timeoutMs = 30_000,
): Promise<void> {
  try {
    await page.waitForFunction(
      ({ id, value }: { id: string; value: string }) => {
        const select = Array.from(document.querySelectorAll("select")).find(
          (candidate) => candidate.getAttribute("data-testid") === id,
        ) as HTMLSelectElement | undefined;
        return select?.value === value;
      },
      { id: testId, value: expected },
      { timeout: timeoutMs, polling: 100 },
    );
  } catch (error) {
    throw new Error(
      `${stage} did not select ${expected} within ${Math.round(timeoutMs / 1000)} seconds`,
      { cause: error },
    );
  }
}

export interface ServerDiagnostics {
  status: string;
  elapsedMs?: number;
  stage: string | null;
  fraction: number | null;
  consoleLines: string[];
}

export function formatError(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

function redactDiagnosticText(text: string): string {
  return text.replace(
    /((?:password|authorization|cookie|token|secret)\s*[:=]\s*)([^\s,;}]+)/gi,
    "$1<redacted>",
  );
}

async function safeAttribute(
  page: Page,
  testId: string,
  attribute: string,
): Promise<string | null> {
  try {
    return await page.getByTestId(testId).getAttribute(attribute);
  } catch {
    return null;
  }
}

/** Read the lifecycle/provisioning details currently exposed by the page. */
export async function readServerDiagnostics(page: Page): Promise<ServerDiagnostics> {
  const status = (await safeAttribute(page, "server-status", "data-status")) ?? "unknown";
  const stage =
    (await safeAttribute(page, "server-activity", "data-progress-stage")) ??
    (await safeAttribute(page, "server-activity", "data-stage"));
  const rawFraction = await safeAttribute(page, "server-activity", "data-fraction");
  const parsedFraction = rawFraction === null ? null : Number(rawFraction);
  const fraction =
    parsedFraction !== null && Number.isFinite(parsedFraction) ? parsedFraction : null;
  let consoleLines: string[] = [];
  try {
    consoleLines = await page.getByTestId("server-console-lines").evaluate((node) =>
      Array.from(node.children)
        .map((child) => child.textContent?.trim() ?? "")
        .filter(Boolean)
        .slice(-5),
    );
  } catch {
    // The console can be unavailable during initial navigation or teardown.
  }

  return {
    status,
    stage,
    fraction,
    consoleLines: consoleLines.map(redactDiagnosticText),
  };
}

function mergeDiagnostics(
  current: ServerDiagnostics,
  previous: ServerDiagnostics | null,
): ServerDiagnostics {
  if (!previous) return current;
  return {
    ...current,
    stage: current.stage ?? previous.stage,
    fraction: current.fraction ?? previous.fraction,
    consoleLines: current.consoleLines.length > 0 ? current.consoleLines : previous.consoleLines,
  };
}

export function formatServerDiagnostics(diagnostics: ServerDiagnostics): string {
  const elapsed =
    diagnostics.elapsedMs === undefined
      ? "unknown"
      : `${Math.round(diagnostics.elapsedMs / 1000)}s`;
  const stage = diagnostics.stage ?? "unknown";
  const fraction = diagnostics.fraction === null ? "unknown" : String(diagnostics.fraction);
  const console = diagnostics.consoleLines.length
    ? diagnostics.consoleLines.join("\n")
    : "unavailable";

  return [
    `Current server status: ${diagnostics.status}`,
    `Elapsed time: ${elapsed}`,
    `Last progress stage: ${stage}`,
    `Last progress fraction: ${fraction}`,
    `Last console output:\n${console}`,
  ].join("\n");
}

/**
 * Wait until the server-detail status enters one of the accepted lifecycle states.
 *
 * Reads the localization-independent `data-status` attribute exposed by
 * `data-testid="server-status"`. A crash is terminal and fails immediately.
 */
export async function waitForServerLifecycle(
  page: Page,
  statuses: string[] = ["preparing", "starting", "online"],
  timeoutMs = 60_000,
): Promise<string> {
  const started = Date.now();
  try {
    const handle = await page.waitForFunction(
      (expected: string[]) => {
        const status = document
          .querySelector('[data-testid="server-status"]')
          ?.getAttribute("data-status");
        if (status === "crashed") return { status };
        if (typeof status !== "string") return null;
        return expected.includes(status) ? { status } : null;
      },
      statuses,
      // Timer polling, not rAF: a busy renderer (progress streaming,
      // animations, software video encode) can starve rAF for seconds,
      // delaying the observation long after the UI actually flipped.
      { timeout: timeoutMs, polling: 100 },
    );

    const result = (await handle.jsonValue()) as { status?: string } | null;
    const status = result?.status ?? "unknown";
    if (status === "crashed") {
      const diagnostics = await readServerDiagnostics(page);
      throw new Error(
        `Server crashed after Start.\n${formatServerDiagnostics({
          ...diagnostics,
          elapsedMs: Date.now() - started,
        })}`,
      );
    }
    console.log(`[demo] status: ${status}`);
    return status;
  } catch (error) {
    if (error instanceof Error && error.message.startsWith("Server crashed after Start.")) {
      throw error;
    }
    const diagnostics = await readServerDiagnostics(page);
    throw new Error(
      `server did not enter ${statuses.join("/")} within ${Math.round(timeoutMs / 1000)} seconds after Start.\n${formatServerDiagnostics({
        ...diagnostics,
        elapsedMs: Date.now() - started,
      })}`,
      { cause: error },
    );
  }
}

/**
 * Wait for the real server to become online, failing immediately on crash.
 *
 * `onStatus` is only an observation hook for chapter markers; it does not
 * change the synchronization condition or generate UI events.
 */
export async function waitForServerOnline(
  page: Page,
  timeoutMs: number,
  onStatus?: (status: string) => void,
): Promise<void> {
  const started = Date.now();
  let previousDiagnostics: ServerDiagnostics | null = null;
  let previousStatus: string | null = null;

  const observe = async (): Promise<void> => {
    const diagnostics = await readServerDiagnostics(page);
    previousDiagnostics = diagnostics;
    if (diagnostics.status !== "unknown" && diagnostics.status !== previousStatus) {
      previousStatus = diagnostics.status;
      onStatus?.(diagnostics.status);
    }
  };

  await observe().catch(() => {});
  const monitor = setInterval(() => {
    void observe().catch(() => {});
  }, 250);

  try {
    const handle = await page.waitForFunction(
      () => {
        const status = document
          .querySelector('[data-testid="server-status"]')
          ?.getAttribute("data-status");
        if (status === "online" || status === "crashed") return { status };
        return null;
      },
      undefined,
      { timeout: timeoutMs, polling: 250 },
    );
    const result = (await handle.jsonValue()) as { status?: string } | null;
    const diagnostics = mergeDiagnostics(await readServerDiagnostics(page), previousDiagnostics);
    const elapsedMs = Date.now() - started;

    if (result?.status === "crashed") {
      throw new Error(
        `Server crashed during first boot.\n${formatServerDiagnostics({
          ...diagnostics,
          elapsedMs,
        })}`,
      );
    }
    if (result?.status !== "online") {
      throw new Error(
        `Server online waiter ended with status ${result?.status ?? "unknown"}.\n${formatServerDiagnostics({
          ...diagnostics,
          elapsedMs,
        })}`,
      );
    }
    console.log(`[demo] status: online`);
  } catch (error) {
    if (error instanceof Error && error.message.startsWith("Server crashed during first boot.")) {
      throw error;
    }

    const diagnostics = mergeDiagnostics(await readServerDiagnostics(page), previousDiagnostics);
    throw new Error(
      `Server did not reach online within ${Math.round(timeoutMs / 1000)} seconds.\n${formatServerDiagnostics({
        ...diagnostics,
        elapsedMs: Date.now() - started,
      })}`,
      { cause: error },
    );
  } finally {
    clearInterval(monitor);
  }
}
