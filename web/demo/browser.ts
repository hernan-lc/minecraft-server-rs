import { mkdir, rm } from "node:fs/promises";
import type { Browser, BrowserContext, Page } from "playwright";
import { chromium } from "playwright";
import { installDemoCursor } from "./cursor.js";
import {
  createRecordingStagingPath,
  recordingDir,
  VIDEO_HEIGHT,
  VIDEO_WIDTH,
} from "./recording.js";
import type { DemoConfig } from "./config.js";
import {
  installNetworkDiagnostics,
  type NetworkDiagnostics,
} from "./networkDiagnostics.js";

export interface DemoBrowser {
  browser: Browser;
  context: BrowserContext;
  page: Page;
  networkDiagnostics: NetworkDiagnostics;
  /** Wall-clock ms when the page (and its video recording) was created. */
  startedAt: number;
  recording: {
    stagingPath: string;
    active: boolean;
  };
}

/**
 * Launch Chromium with a high-quality Playwright screencast enabled.
 *
 * The page owns one `.webm` staging file from before the first navigation.
 * No API keys, accounts, or cloud sessions are involved.
 */
export async function launchDemoBrowser(config: DemoConfig): Promise<DemoBrowser> {
  const browser = await chromium.launch({ headless: config.headless });
  let stagingPath: string | null = null;
  try {
    const context = await browser.newContext({
      viewport: { width: VIDEO_WIDTH, height: VIDEO_HEIGHT },
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
      const networkDiagnostics = installNetworkDiagnostics(page);
      // Installed before the first navigation so the cursor is present in
      // every document, including after full-page redirects.
      await installDemoCursor(page);
      await mkdir(recordingDir(), { recursive: true });
      stagingPath = createRecordingStagingPath();
      await page.screencast.start({
        path: stagingPath,
        size: { width: VIDEO_WIDTH, height: VIDEO_HEIGHT },
        quality: config.videoQuality,
      });
      return {
        browser,
        context,
        page,
        networkDiagnostics,
        startedAt: Date.now(),
        recording: { stagingPath, active: true },
      };
    } catch (error) {
      await context.close();
      if (stagingPath) await rm(stagingPath, { force: true }).catch(() => {});
      throw error;
    }
  } catch (error) {
    await browser.close();
    throw error;
  }
}

/** Close everything; safe to call after a partial launch failure. */
export async function closeDemoBrowser(handle: Partial<DemoBrowser>): Promise<void> {
  if (handle.page && handle.recording?.active) {
    try {
      await handle.page.screencast.stop();
    } catch {
      // Preserve the workflow error; browser cleanup is best effort.
    } finally {
      handle.recording.active = false;
    }
  }
  if (handle.page) await handle.page.close().catch(() => {});
  if (handle.context) await handle.context.close().catch(() => {});
  if (handle.browser) await handle.browser.close().catch(() => {});
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
