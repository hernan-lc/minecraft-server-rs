import type { Locator, Page } from "playwright";
import type { DemoConfig } from "./config.js";
import { demoPause, pause } from "./helpers.js";

/**
 * Synthetic demo cursor, injected from Playwright.
 *
 * `locator.click()` is visually robotic in recordings: nothing travels
 * across the screen. This module renders a tutorial-style pointer inside
 * the page (white center, dark outline, click ripple) and drives the real
 * Playwright mouse to each control with visible multi-step movement, so
 * the WebM shows a cursor that travels and clicks.
 *
 * Production application code is never touched: everything is installed
 * via `page.addInitScript()`, which re-installs after full navigations.
 */

/** Install (or re-confirm) the cursor; safe to call on every navigation. */
export async function installDemoCursor(page: Page): Promise<void> {
  // A plain string, not a function: this file is transformed by tsx/esbuild
  // with keepNames, which wraps nested function declarations in a `__name()`
  // helper. Playwright serializes only the passed function, so the helper
  // is missing in the page and the install throws `__name is not defined`.
  // String contents pass through the bundler untouched.
  const source = `
    (function () {
      function install() {
        if (document.querySelector("[data-mcpanel-demo-cursor]")) return;
        var style = document.createElement("style");
        style.textContent = [
          "[data-mcpanel-demo-cursor] {",
          "  position: fixed; left: 0; top: 0; width: 20px; height: 20px;",
          "  border-radius: 9999px;",
          "  border: 2px solid rgba(255,255,255,.95);",
          "  background: rgba(20,20,20,.55);",
          "  box-shadow: 0 2px 8px rgba(0,0,0,.5), 0 0 0 1px rgba(0,0,0,.8);",
          "  pointer-events: none; z-index: 2147483647;",
          "  transform: translate(-50%, -50%);",
          "  transition: transform 100ms ease;",
          "}",
          "[data-mcpanel-demo-cursor].clicking {",
          "  transform: translate(-50%, -50%) scale(.75);",
          "}",
          "[data-mcpanel-demo-ripple] {",
          "  position: fixed; width: 42px; height: 42px;",
          "  border: 2px solid rgba(255,255,255,.75);",
          "  border-radius: 9999px;",
          "  pointer-events: none; z-index: 2147483646;",
          "  transform: translate(-50%, -50%) scale(.4);",
          "  opacity: 1;",
          "  animation: mcpanel-demo-ripple 320ms ease-out forwards;",
          "}",
          "@keyframes mcpanel-demo-ripple {",
          "  to { transform: translate(-50%, -50%) scale(1); opacity: 0; }",
          "}",
        ].join("\\n");
        document.head.appendChild(style);

        var cursor = document.createElement("div");
        cursor.setAttribute("data-mcpanel-demo-cursor", "true");
        cursor.style.left = "-100px";
        cursor.style.top = "-100px";
        // Coalesce position writes to one per frame: a stepped mouse.move
        // fires many mousemove events, and a style recalc per event janks
        // the page (and the recording) on busy screens.
        var pendingX = -100;
        var pendingY = -100;
        var frameQueued = false;
        var mount = function () { document.body.appendChild(cursor); };
        if (document.body) {
          mount();
        } else {
          document.addEventListener("DOMContentLoaded", mount, { once: true });
        }

        document.addEventListener("mousemove", function (event) {
          pendingX = event.clientX;
          pendingY = event.clientY;
          if (!frameQueued) {
            frameQueued = true;
            window.requestAnimationFrame(function () {
              frameQueued = false;
              cursor.style.left = pendingX + "px";
              cursor.style.top = pendingY + "px";
            });
          }
        });
        document.addEventListener("mousedown", function (event) {
          cursor.classList.add("clicking");
          var ripple = document.createElement("div");
          ripple.setAttribute("data-mcpanel-demo-ripple", "true");
          ripple.style.left = event.clientX + "px";
          ripple.style.top = event.clientY + "px";
          document.body.appendChild(ripple);
          window.setTimeout(function () { ripple.remove(); }, 350);
        });
        document.addEventListener("mouseup", function () {
          cursor.classList.remove("clicking");
        });
      }

      if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", install, { once: true });
      } else {
        install();
      }
    })();
  `;
  await page.addInitScript(source);
}

/** True when the cursor element is present in the current document. */
export async function hasDemoCursor(page: Page): Promise<boolean> {
  return await page
    .evaluate(
      () => document.querySelector("[data-mcpanel-demo-cursor]") !== null,
    )
    .catch(() => false);
}

/**
 * Smoothly travel the physical mouse to the center of a control.
 *
 * The movement itself is part of the demo — never teleport with a single
 * `mouse.click(x, y)`.
 */
export async function moveToLocator(page: Page, locator: Locator): Promise<void> {
  await locator.scrollIntoViewIfNeeded();
  const box = await locator.boundingBox();
  if (!box) {
    throw new Error("Cannot move demo cursor: target has no bounding box");
  }
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2, {
    steps: 10,
  });
}

/**
 * Human-like click: travel, settle, press, release.
 * The in-page cursor renders the travel and the click ripple.
 */
export async function demoClick(
  page: Page,
  locator: Locator,
  config: DemoConfig,
): Promise<void> {
  // Verify the target before moving the physical mouse. This catches a
  // disabled, covered, or stale target before the recording shows a missed
  // click.
  await locator.click({ trial: true });
  await moveToLocator(page, locator);
  await demoPause(config, "short");

  // The pause can allow a responsive layout to reflow. Revalidate and read a
  // fresh box immediately before pressing the physical mouse button.
  await locator.click({ trial: true });
  const box = await locator.boundingBox();
  if (!box) {
    throw new Error("Demo click target disappeared before click.");
  }
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;
  await page.mouse.move(x, y, { steps: 3 });
  await page.mouse.down();
  await pause(100);
  await page.mouse.up();
  await demoPause(config, "short");
}

/** Travel to a control and focus it (for inputs filled right after). */
export async function demoFocus(
  page: Page,
  locator: Locator,
  config: DemoConfig,
): Promise<void> {
  await locator.click({ trial: true });
  await moveToLocator(page, locator);
  await demoPause(config, "short");
  await locator.click({ trial: true });
  await locator.focus();
}

/** Travel, click, and type at human-readable speed. Never for passwords. */
export async function demoType(
  page: Page,
  locator: Locator,
  value: string,
  config: DemoConfig,
  delayMs = 45,
): Promise<void> {
  await demoClick(page, locator, config);
  // Clear first: pressSequentially appends, and some fields (e.g. the setup
  // username) already carry a default value.
  await locator.fill("");
  await locator.pressSequentially(value, { delay: delayMs });
}

/** Travel to a native select, change it, and hold the new value on screen. */
export async function demoSelectOption(
  page: Page,
  locator: Locator,
  value: string,
  config: DemoConfig,
): Promise<void> {
  await locator.click({ trial: true });
  await moveToLocator(page, locator);
  await demoPause(config, "short");
  await locator.click({ trial: true });
  await locator.selectOption(value);
  await demoPause(config, "short");
}
