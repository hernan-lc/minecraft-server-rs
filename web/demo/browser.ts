import type { Browser, BrowserContext, Page } from "playwright";
import { chromium } from "playwright";
import { recordingDir, VIDEO_HEIGHT, VIDEO_WIDTH } from "./recording.js";
import type { DemoConfig } from "./config.js";

export interface DemoBrowser {
  browser: Browser;
  context: BrowserContext;
  page: Page;
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
      const page = await context.newPage();
      return { browser, context, page };
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
