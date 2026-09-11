import type { Page } from "playwright";

export const CAPTURE_HEARTBEAT_INTERVAL_MS = 750;
export const CAPTURE_HEARTBEAT_TEST_ID = "mcpanel-demo-capture-heartbeat";

export interface DisposableCaptureHeartbeat {
  stop(): Promise<void>;
}

const HEARTBEAT_STATE_KEY = "__mcpanelDemoCaptureHeartbeat";

/**
 * Keep Chromium producing real browser paints during long, otherwise idle
 * waits. This never moves the mouse, dispatches input, or changes app state.
 */
export async function startCaptureHeartbeat(page: Page): Promise<DisposableCaptureHeartbeat> {
  await page.evaluate(
    ({ intervalMs, testId, stateKey }) => {
      type HeartbeatState = { interval: number; node: HTMLElement };
      const windowState = window as unknown as Record<string, HeartbeatState | undefined>;
      const previous = windowState[stateKey];
      if (previous) {
        window.clearInterval(previous.interval);
        previous.node.remove();
      }

      const node = document.createElement("div");
      node.setAttribute("data-mcpanel-demo-capture-heartbeat", testId);
      node.style.position = "fixed";
      node.style.right = "0";
      node.style.bottom = "0";
      node.style.width = "1px";
      node.style.height = "1px";
      node.style.pointerEvents = "none";
      node.style.zIndex = "2147483647";
      node.style.backgroundColor = "rgba(255,255,255,0.01)";
      document.body.appendChild(node);

      let toggle = false;
      const interval = window.setInterval(() => {
        toggle = !toggle;
        node.style.backgroundColor = toggle
          ? "rgba(255,255,255,0.01)"
          : "rgba(0,0,0,0.01)";
      }, intervalMs);

      windowState[stateKey] = { interval, node };
    },
    {
      intervalMs: CAPTURE_HEARTBEAT_INTERVAL_MS,
      testId: CAPTURE_HEARTBEAT_TEST_ID,
      stateKey: HEARTBEAT_STATE_KEY,
    },
  );

  let stopped = false;
  return {
    async stop(): Promise<void> {
      if (stopped) return;
      stopped = true;
      await page
        .evaluate((stateKey) => {
          type HeartbeatState = { interval: number; node: HTMLElement };
          const windowState = window as unknown as Record<string, HeartbeatState | undefined>;
          const state = windowState[stateKey];
          if (!state) return;
          window.clearInterval(state.interval);
          state.node.remove();
          delete windowState[stateKey];
        }, HEARTBEAT_STATE_KEY)
        .catch(() => {
          // Page cleanup may already have happened after a workflow failure.
        });
    },
  };
}
