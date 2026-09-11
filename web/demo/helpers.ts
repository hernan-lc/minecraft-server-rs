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
  // Tuned so the full demo lands at 15–20 s: cursor travel (~0.3 s per
  // control) and app latency (hashing, catalog fetch) already consume
  // ~10 s, leaving ~9 s for deliberate pauses.
  short: 200,
  normal: 400,
  long: 800,
  // States the viewer needs to understand: server card, final lifecycle.
  reveal: 1000,
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
  timeoutMs = 15_000,
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

/** Wait until a locator is detached from the DOM (e.g. modal closed). */
export async function waitForGone(
  locator: Locator,
  stage: string,
  timeoutMs = 15_000,
): Promise<void> {
  try {
    await locator.first().waitFor({ state: "detached", timeout: timeoutMs });
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
  timeoutMs = 30_000,
): Promise<string[]> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const options = await page
      .getByTestId(testId)
      .evaluate((select: HTMLSelectElement) =>
        Array.from(select.options).map((o) => o.value).filter((v) => v !== ""),
      )
      .catch(() => [] as string[]);
    if (options.length > 0) return options;
    if (Date.now() >= deadline) {
      throw new Error(
        `${stage} did not populate within ${Math.round(timeoutMs / 1000)} seconds`,
      );
    }
    await pause(250);
  }
}

/**
 * Wait until the server-detail status leaves the stopped state.
 *
 * Reads the localization-independent `data-status` attribute exposed by
 * `data-testid="server-status"`. Never waits for full Minecraft
 * provisioning — `preparing` is enough to prove the lifecycle began.
 */
export async function waitForServerLifecycle(
  page: Page,
  statuses: string[] = ["preparing", "starting", "running"],
  timeoutMs = 60_000,
): Promise<string> {
  try {
    await page.waitForFunction(
      (expected: string[]) => {
        const status = document
          .querySelector('[data-testid="server-status"]')
          ?.getAttribute("data-status");
        return status !== null && expected.includes(status ?? "");
      },
      statuses,
      // Timer polling, not rAF: a busy renderer (progress streaming,
      // animations, software video encode) can starve rAF for seconds,
      // delaying the observation long after the UI actually flipped.
      { timeout: timeoutMs, polling: 100 },
    );
  } catch (error) {
    throw new Error(
      `server did not enter ${statuses.join("/")} within ${Math.round(timeoutMs / 1000)} seconds after Start`,
      { cause: error },
    );
  }
  const status =
    (await page
      .getByTestId("server-status")
      .getAttribute("data-status")
      .catch(() => null)) ?? "unknown";
  console.log(`[demo] status: ${status}`);
  return status;
}
