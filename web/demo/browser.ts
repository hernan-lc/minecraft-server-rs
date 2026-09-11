import type { Browser, BrowserContext, Page } from "playwright";
import { chromium } from "playwright";
import { installDemoCursor } from "./cursor.js";
import { recordingDir, VIDEO_HEIGHT, VIDEO_WIDTH } from "./recording.js";
import type { DemoConfig } from "./config.js";

export interface DemoBrowser {
  browser: Browser;
  context: BrowserContext;
  page: Page;
  /** Wall-clock ms when the page (and its video recording) was created. */
  startedAt: number;
}

/**
 * Launch Chromium with video recording enabled.
 *
 * Recording is a context-level Playwright feature: every page in the
 * context is captured to a `.webm` file, finalized when the page closes.
 * No API keys, accounts, or cloud sessions are involved.
 */
export async function launchDemoBrowser(config: DemoConfig): Promise<DemoBrowser> {
  const browser = await chromium.launch({ headless: config.headless });
  try {
    const context = await browser.newContext({
      viewport: { width: VIDEO_WIDTH, height: VIDEO_HEIGHT },
      recordVideo: {
        dir: recordingDir(),
        size: { width: VIDEO_WIDTH, height: VIDEO_HEIGHT },
      },
    });
    try {
      // The panel serves a strict `style-src 'self'` CSP, which silently
      // blocks the demo cursor's injected <style> element and inline
      // positioning. Relax style-src to 'unsafe-inline' for document
      // responses in the DEMO context only, so the injected cursor renders.
      // Production headers are untouched.
      //
      // Routing EVERY request through a handler measurably slows live
      // traffic (websocket console stream, API polling), so first-run
      // disables interception once the last full navigation is done — see
      // disableRequestInterception(). Later demos with their own full
      // navigations must re-enable it the same way.
      await enableCspCursorWorkaround(context);
      const page = await context.newPage();
      // Installed before the first navigation so the cursor is present in
      // every document, including after full-page redirects.
      await installDemoCursor(page);
      return { browser, context, page, startedAt: Date.now() };
    } catch (error) {
      await context.close();
      throw error;
    }
  } catch (error) {
    await browser.close();
    throw error;
  }
}

/** Close everything; safe to call after a partial launch failure. */
export async function closeDemoBrowser(handle: Partial<DemoBrowser>): Promise<void> {
  // The page is closed by recording finalization; closing the context and
  // browser here covers every partial-initialization path.
  await handle.context?.close().catch(() => {});
  await handle.browser?.close().catch(() => {});
}

async function enableCspCursorWorkaround(context: BrowserContext): Promise<void> {
  await context.route("**/*", async (route) => {
    if (route.request().resourceType() !== "document") {
      await route.continue();
      return;
    }
    const response = await route.fetch();
    const headers: Record<string, string> = { ...response.headers };
    const csp = headers["content-security-policy"];
    if (typeof csp === "string" && csp.includes("style-src")) {
      headers["content-security-policy"] = csp.replace(
        "style-src 'self'",
        "style-src 'self' 'unsafe-inline'",
      );
    }
    await route.fulfill({ response, headers });
  });
}

/**
 * Remove all request interception from the context.
 *
 * Call once no more full-page navigations will happen: the cursor styles
 * are already applied to every loaded document, and live traffic (API
 * polling, console websocket) must not pay the interception overhead.
 */
export async function disableRequestInterception(
  context: BrowserContext,
): Promise<void> {
  await context.unrouteAll({ behavior: "wait" }).catch(() => {});
}
