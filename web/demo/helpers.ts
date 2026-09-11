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

export type ConsoleConnectionState =
  | "connected"
  | "reconnecting"
  | "disconnected"
  | "unknown";

export interface DiagnosticLogLine {
  stream: string;
  line: string;
}

export interface BackendServerDiagnostics {
  serverId: string;
  serverHttpStatus: number | null;
  logsHttpStatus: number | null;
  status: string | null;
  pid: number | null;
  uptimeSecs: number | null;
  stage: string | null;
  fraction: number | null;
  logLines: DiagnosticLogLine[];
  error: string | null;
}

export interface ServerDiagnostics {
  /** Status rendered by ServerDetail. Kept as `status` for old callers. */
  uiStatus: string;
  status: string;
  /** Backend state is authoritative for diagnosing a long startup. */
  backendStatus: string | null;
  serverId: string | null;
  pid: number | null;
  uptimeSecs: number | null;
  elapsedMs?: number;
  stage: string | null;
  fraction: number | null;
  consoleConnection: ConsoleConnectionState;
  uiConsoleLines: string[];
  backendConsoleLines: DiagnosticLogLine[];
  /** Compatibility alias for the former DOM-only diagnostics shape. */
  consoleLines: string[];
  serverHttpStatus: number | null;
  logsHttpStatus: number | null;
  backendError: string | null;
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

function redactDiagnosticError(text: string): string {
  return redactDiagnosticText(text).slice(0, 500);
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

/** Read the stable backend identity rendered by the current ServerDetail. */
export async function readServerId(page: Page): Promise<string | null> {
  const id = await safeAttribute(page, "server-detail", "data-server-id");
  return id?.trim() || null;
}

function recordValue(value: unknown, key: string): unknown {
  if (typeof value !== "object" || value === null) return undefined;
  return (value as Record<string, unknown>)[key];
}

function stringValue(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function numberValue(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function progressValue(value: unknown): { stage: string | null; fraction: number | null } {
  const stage = stringValue(recordValue(value, "stage"));
  const rawFraction = numberValue(recordValue(value, "fraction"));
  return {
    stage,
    fraction: rawFraction === null ? null : Math.max(0, Math.min(1, rawFraction)),
  };
}

function diagnosticLines(value: unknown): DiagnosticLogLine[] {
  if (!Array.isArray(value)) return [];
  return value
    .map((entry): DiagnosticLogLine | null => {
      const stream = stringValue(recordValue(entry, "stream"));
      const line = stringValue(recordValue(entry, "line"));
      if (!stream || !line) return null;
      return { stream, line: redactDiagnosticText(line) };
    })
    .filter((entry): entry is DiagnosticLogLine => entry !== null)
    .slice(-50);
}

/**
 * Fetch the same-origin REST state and retained Guardian console independently
 * of the browser console WebSocket. This deliberately returns a safe subset
 * of the API response rather than retaining an arbitrary backend object.
 */
export async function readBackendServerDiagnostics(
  page: Page,
  serverId: string,
): Promise<BackendServerDiagnostics> {
  const raw = await page.evaluate(async ({ id }) => {
    try {
      const encoded = encodeURIComponent(id);
      const [serverResponse, logsResponse] = await Promise.all([
        fetch(`/api/servers/${encoded}`, { credentials: "same-origin" }),
        fetch(`/api/servers/${encoded}/logs`, { credentials: "same-origin" }),
      ]);
      const [server, logs] = await Promise.all([
        serverResponse.ok ? serverResponse.json().catch(() => null) : Promise.resolve(null),
        logsResponse.ok ? logsResponse.json().catch(() => []) : Promise.resolve([]),
      ]);
      return {
        serverHttpStatus: serverResponse.status,
        logsHttpStatus: logsResponse.status,
        server,
        logs,
        error: null,
      };
    } catch (error) {
      return {
        serverHttpStatus: null,
        logsHttpStatus: null,
        server: null,
        logs: [],
        error: error instanceof Error ? error.message : String(error),
      };
    }
  }, { id: serverId });

  const server = recordValue(raw, "server");
  const progress = progressValue(recordValue(server, "progress"));
  return {
    serverId,
    serverHttpStatus: numberValue(recordValue(raw, "serverHttpStatus")),
    logsHttpStatus: numberValue(recordValue(raw, "logsHttpStatus")),
    status: stringValue(recordValue(server, "status")),
    pid: numberValue(recordValue(server, "pid")),
    uptimeSecs: numberValue(recordValue(server, "uptime_secs")),
    stage: progress.stage,
    fraction: progress.fraction,
    logLines: diagnosticLines(recordValue(raw, "logs")),
    error: stringValue(recordValue(raw, "error"))
      ? redactDiagnosticError(stringValue(recordValue(raw, "error")) ?? "")
      : null,
  };
}

function validConnectionState(value: string | null): ConsoleConnectionState {
  return value === "connected" || value === "reconnecting" || value === "disconnected"
    ? value
    : "unknown";
}

async function readUiConsoleLines(page: Page): Promise<string[]> {
  try {
    const locator = page.getByTestId("server-console-lines").locator("[data-console-line]");
    const lines = await locator.allTextContents();
    if (lines.length > 0) return lines.map((line) => redactDiagnosticText(line.trim())).filter(Boolean).slice(-50);
  } catch {
    // Older or partially mounted console markup falls through to the safe
    // text-only fallback below.
  }
  try {
    return await page
      .getByTestId("server-console-lines")
      .evaluate((node) =>
        Array.from(node.children)
          .filter((child) => child.hasAttribute("data-console-line"))
          .map((child) => child.textContent?.trim() ?? "")
          .filter(Boolean)
          .slice(-50),
      )
      .then((lines) => lines.map(redactDiagnosticText));
  } catch {
    return [];
  }
}

/** Read the lifecycle/provisioning details currently exposed by the page. */
export async function readServerDiagnostics(page: Page): Promise<ServerDiagnostics> {
  const uiStatus = (await safeAttribute(page, "server-status", "data-status")) ?? "unknown";
  const uiStage =
    (await safeAttribute(page, "server-activity", "data-progress-stage")) ??
    (await safeAttribute(page, "server-activity", "data-stage"));
  const rawFraction = await safeAttribute(page, "server-activity", "data-fraction");
  const parsedFraction = rawFraction === null ? null : Number(rawFraction);
  const fraction =
    parsedFraction !== null && Number.isFinite(parsedFraction) ? parsedFraction : null;
  const uiConsoleLines = await readUiConsoleLines(page);
  const consoleConnection = validConnectionState(
    await safeAttribute(page, "server-console-connection", "data-state"),
  );
  const serverId = await readServerId(page);
  let backend: BackendServerDiagnostics | null = null;
  if (serverId) {
    try {
      backend = await readBackendServerDiagnostics(page, serverId);
    } catch (error) {
      backend = {
        serverId,
        serverHttpStatus: null,
        logsHttpStatus: null,
        status: null,
        pid: null,
        uptimeSecs: null,
        stage: null,
        fraction: null,
        logLines: [],
        error: redactDiagnosticError(formatError(error)),
      };
    }
  }
  const backendConsoleLines = backend?.logLines ?? [];
  const status = uiStatus;

  return {
    uiStatus,
    status,
    backendStatus: backend?.status ?? null,
    serverId,
    pid: backend?.pid ?? null,
    uptimeSecs: backend?.uptimeSecs ?? null,
    stage: backend?.stage ?? uiStage,
    fraction: backend?.fraction ?? fraction,
    consoleConnection,
    uiConsoleLines,
    backendConsoleLines,
    consoleLines: uiConsoleLines,
    serverHttpStatus: backend?.serverHttpStatus ?? null,
    logsHttpStatus: backend?.logsHttpStatus ?? null,
    backendError: backend?.error ?? null,
  };
}

function mergeDiagnostics(
  current: ServerDiagnostics,
  previous: ServerDiagnostics | null,
): ServerDiagnostics {
  if (!previous) return current;
  return {
    ...current,
    uiStatus: current.uiStatus === "unknown" ? previous.uiStatus : current.uiStatus,
    status: current.uiStatus === "unknown" ? previous.status : current.status,
    backendStatus: current.backendStatus ?? previous.backendStatus,
    serverId: current.serverId ?? previous.serverId,
    pid: current.pid ?? previous.pid,
    uptimeSecs: current.uptimeSecs ?? previous.uptimeSecs,
    stage: current.stage ?? previous.stage,
    fraction: current.fraction ?? previous.fraction,
    consoleConnection:
      current.consoleConnection === "unknown" ? previous.consoleConnection : current.consoleConnection,
    uiConsoleLines: current.uiConsoleLines.length > 0 ? current.uiConsoleLines : previous.uiConsoleLines,
    backendConsoleLines:
      current.backendConsoleLines.length > 0 ? current.backendConsoleLines : previous.backendConsoleLines,
    consoleLines: current.uiConsoleLines.length > 0 ? current.uiConsoleLines : previous.consoleLines,
    serverHttpStatus: current.serverHttpStatus ?? previous.serverHttpStatus,
    logsHttpStatus: current.logsHttpStatus ?? previous.logsHttpStatus,
    backendError: current.backendError ?? previous.backendError,
  };
}

/** Return a layer-specific diagnosis for a failed first boot. */
export function classifyStartupFailure(diagnostics: ServerDiagnostics): string {
  const processLines = diagnostics.backendConsoleLines.filter((entry) => entry.stream !== "system");
  const hasDone = processLines.some((entry) => /Done \([^\n]*\).*For help, type/i.test(entry.line));

  if (
    diagnostics.backendConsoleLines.length > 0 &&
    diagnostics.uiConsoleLines.length === 0 &&
    diagnostics.consoleConnection !== "connected"
  ) {
    return "Console transport failure: Guardian has console output but the browser WebSocket is disconnected.";
  }
  if (
    diagnostics.pid !== null &&
    diagnostics.backendConsoleLines.length > 0 &&
    processLines.length === 0
  ) {
    return "Java process was spawned, but Guardian is not receiving Minecraft stdout/stderr. Inspect child pipe handling.";
  }
  if (hasDone && diagnostics.backendStatus === "starting") {
    return "Guardian readiness detection failure: Paper emitted its ready marker but status was not promoted to Online.";
  }
  if (diagnostics.backendStatus === "online" && diagnostics.uiStatus !== "online") {
    return `Frontend status synchronization failure: backend is Online but ServerDetail still shows ${diagnostics.uiStatus}.`;
  }
  if (diagnostics.backendStatus === "starting" && diagnostics.pid !== null && !hasDone) {
    return "Minecraft process is alive but has not completed startup.";
  }
  return "No single startup layer was identified from the captured diagnostics.";
}

export function formatServerDiagnostics(diagnostics: ServerDiagnostics): string {
  const elapsed =
    diagnostics.elapsedMs === undefined
      ? "unknown"
      : `${Math.round(diagnostics.elapsedMs / 1000)}s`;
  const stage = diagnostics.stage ?? "unknown";
  const fraction = diagnostics.fraction === null ? "unknown" : String(diagnostics.fraction);
  const uiConsole = diagnostics.uiConsoleLines.length
    ? diagnostics.uiConsoleLines.join("\n")
    : "unavailable";
  const backendConsole = diagnostics.backendConsoleLines.length
    ? diagnostics.backendConsoleLines
        .map((entry) => `[${entry.stream}] ${entry.line}`)
        .join("\n")
    : "unavailable";

  return [
    `UI status: ${diagnostics.uiStatus}`,
    `Backend status: ${diagnostics.backendStatus ?? "unavailable"}`,
    `PID: ${diagnostics.pid ?? "unavailable"}`,
    `Uptime: ${diagnostics.uptimeSecs === null ? "unavailable" : `${diagnostics.uptimeSecs}s`}`,
    `Elapsed time: ${elapsed}`,
    `Last progress stage: ${stage}`,
    `Last progress fraction: ${fraction}`,
    `Console transport: ${diagnostics.consoleConnection}`,
    `Guardian backend logs: ${diagnostics.backendConsoleLines.length}`,
    `UI console lines: ${diagnostics.uiConsoleLines.length}`,
    `Last backend lines:\n${backendConsole}`,
    `Last UI console output:\n${uiConsole}`,
    `Likely failure: ${classifyStartupFailure(diagnostics)}`,
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
  onDiagnostics?: (diagnostics: ServerDiagnostics) => void,
): Promise<void> {
  const started = Date.now();
  let previousDiagnostics: ServerDiagnostics | null = null;
  let previousStatus: string | null = null;

  try {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const diagnostics = mergeDiagnostics(await readServerDiagnostics(page), previousDiagnostics);
      previousDiagnostics = diagnostics;
      onDiagnostics?.(diagnostics);
      const authoritativeStatus = diagnostics.backendStatus ?? diagnostics.uiStatus;
      if (authoritativeStatus !== "unknown" && authoritativeStatus !== previousStatus) {
        previousStatus = authoritativeStatus;
        onStatus?.(authoritativeStatus);
      }

      if (authoritativeStatus === "crashed") {
        throw new Error(
          `Server crashed during first boot.\n${formatServerDiagnostics({
            ...diagnostics,
            elapsedMs: Date.now() - started,
          })}`,
        );
      }

      if (authoritativeStatus === "online") {
        const uiDeadline = Math.min(deadline, Date.now() + 15_000);
        let uiDiagnostics = diagnostics;
        while (Date.now() < uiDeadline) {
          uiDiagnostics = mergeDiagnostics(await readServerDiagnostics(page), uiDiagnostics);
          onDiagnostics?.(uiDiagnostics);
          if (uiDiagnostics.uiStatus === "online") {
            console.log("[demo] backend status: online");
            console.log("[demo] UI status: online");
            return;
          }
          if (uiDiagnostics.uiStatus === "crashed") {
            throw new Error(
              `Server crashed during first boot.\n${formatServerDiagnostics({
                ...uiDiagnostics,
                elapsedMs: Date.now() - started,
              })}`,
            );
          }
          await pause(250);
        }
        throw new Error(
          `Frontend status synchronization failure: backend is Online but ServerDetail still shows ${uiDiagnostics.uiStatus}.\n${formatServerDiagnostics({
            ...uiDiagnostics,
            elapsedMs: Date.now() - started,
          })}`,
        );
      }

      await pause(Math.min(1_000, Math.max(1, deadline - Date.now())));
    }

    const diagnostics = previousDiagnostics ?? (await readServerDiagnostics(page));
    throw new Error(
      `Server did not reach online within ${Math.round(timeoutMs / 1000)} seconds.\n${formatServerDiagnostics({
        ...diagnostics,
        elapsedMs: Date.now() - started,
      })}`,
    );
  } catch (error) {
    if (
      error instanceof Error &&
      (error.message.startsWith("Server crashed during first boot.") ||
        error.message.startsWith("Frontend status synchronization failure:"))
    ) {
      throw error;
    }

    const diagnostics = mergeDiagnostics(await readServerDiagnostics(page), previousDiagnostics);
    onDiagnostics?.(diagnostics);
    throw new Error(
      `Server did not reach online within ${Math.round(timeoutMs / 1000)} seconds.\n${formatServerDiagnostics({
        ...diagnostics,
        elapsedMs: Date.now() - started,
      })}`,
      { cause: error },
    );
  }
}
