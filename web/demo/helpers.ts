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

export type DemoPace = "short" | "normal" | "long";

const PACING_MS: Record<DemoPace, number> = {
  short: 250,
  normal: 500,
  long: 1000,
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
  console.log(`[demo] ${name}`);
  try {
    return await operation();
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
