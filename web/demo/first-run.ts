import "dotenv/config";
import type { BrowserContext, Page } from "playwright";
import { closeDemoBrowser, disableRequestInterception, launchDemoBrowser, type DemoBrowser } from "./browser.js";
import { loadDemoConfig, type DemoConfig } from "./config.js";
import {
  demoClick,
  demoFocus,
  demoSelectOption,
  demoType,
  moveToLocator,
} from "./cursor.js";
import {
  assertVisible,
  demoPause,
  pause,
  readBodyText,
  step,
  waitForGone,
  waitForSelectOptions,
  waitForServerLifecycle,
} from "./helpers.js";
import { saveRecording } from "./recording.js";

const DEMO_SERVER_NAME = "Survival";

/**
 * First Run → Create Server → Start, recorded to
 * `artifacts/demos/first-run.webm` (~15–20 s).
 *
 * The story, without narration: configure mcpanel → sign in → create a
 * Paper server → open it → start it → mcpanel begins provisioning it.
 *
 * Two concerns stay separate: Playwright waits synchronize on application
 * state; `demoPause()` + cursor movement exist only for presentation
 * pacing, after the expected state already exists.
 *
 * Everything is deterministic (`data-testid` locators). The password is
 * filled directly — masked and quick, never typed slowly — and never logged.
 */
async function runFirstRun(
  page: Page,
  context: BrowserContext,
  config: DemoConfig,
): Promise<void> {
  await step("opening setup", async () => {
    await page.goto(`${config.baseUrl}/setup`);
    // The setup form is the proof that a fresh data directory is in use.
    // A completed setup renders an "already completed" notice instead.
    try {
      await assertVisible(page.getByTestId("setup-username"), "setup form", 15_000);
    } catch (error) {
      const body = await readBodyText(page).catch(() => "");
      if (/already been completed/i.test(body)) {
        throw new Error(
          "Demo requires a fresh mcpanel data directory. Setup has already been completed.",
        );
      }
      throw error;
    }
  });

  await step("creating admin", async () => {
    await demoType(page, page.getByTestId("setup-username"), config.username, config);
    await demoFocus(page, page.getByTestId("setup-password"), config);
    await page.getByTestId("setup-password").fill(config.password);
    await page.getByTestId("setup-password-confirm").fill(config.password);
    await demoClick(page, page.getByTestId("setup-submit"), config);
    // Setup redirects to / (login). Wait for the login form, not a sleep.
    await assertVisible(page.getByTestId("login-username"), "login form after setup", 15_000);
  });

  await step("logging in", async () => {
    // The username defaults to the admin name; only the password is entered.
    await demoFocus(page, page.getByTestId("login-password"), config);
    await page.getByTestId("login-password").fill(config.password);
    await demoClick(page, page.getByTestId("login-submit"), config);
    await assertVisible(page.getByTestId("new-server"), "dashboard", 15_000);
    // Last full navigation is done (the rest is SPA state): drop request
    // interception so live traffic runs at full speed. Cursor styles are
    // already applied to every loaded document.
    await disableRequestInterception(context);
  });

  await step("opening new server", async () => {
    await demoClick(page, page.getByTestId("new-server"), config);
    await assertVisible(page.getByTestId("server-name"), "new server form", 15_000);
  });

  await step(`creating ${DEMO_SERVER_NAME}`, async () => {
    await demoType(page, page.getByTestId("server-name"), DEMO_SERVER_NAME, config);

    // Core: paper. Native selects are shown, not dropdown-automated.
    await demoSelectOption(page, page.getByTestId("server-core"), "paper", config);

    // Minecraft version: use the latest provided by the actual UI/API —
    // never a hard-coded version. The versions list populates async.
    const versions = await waitForSelectOptions(page, "server-version", "Minecraft versions", 30_000);
    await moveToLocator(page, page.getByTestId("server-version"));
    await demoPause(config, "short");
    const minecraftVersion = await page.getByTestId("server-version").inputValue();
    if (!minecraftVersion) {
      throw new Error("Minecraft versions failed to populate");
    }
    console.log(`[demo] minecraft: ${minecraftVersion} (latest of ${versions.length})`);

    // Java: verify the application's selection rather than overriding it.
    // Current Minecraft 26.x releases expect Java 25; silently flipping
    // Java 21 → 25 here would hide an application regression.
    await moveToLocator(page, page.getByTestId("server-java"));
    await demoPause(config, "short");
    const java = await page.getByTestId("server-java").inputValue();
    console.log(`[demo] java: ${java}`);
    if (/^26(\.|$)/.test(minecraftVersion.trim())) {
      if (java !== "25") {
        throw new Error(
          `expected Java version 25 for Minecraft ${minecraftVersion}, but the application selected Java ${java || "(none)"}`,
        );
      }
    } else if (!java) {
      throw new Error("expected Java version not selected");
    }

    // Port: keep the sensible default unless the app requires otherwise.
    const port = await page.getByTestId("server-port").inputValue().catch(() => "");
    if (port && port !== "25565") {
      console.log(`[demo] port: ${port}`);
    }

    // EULA must be accepted or the server refuses to start. A real mouse
    // click keeps the interaction visible in the recording.
    await demoClick(page, page.getByTestId("server-eula"), config);
    if (!(await page.getByTestId("server-eula").isChecked())) {
      throw new Error("EULA checkbox did not become checked");
    }

    // Visibly travel to Create, then submit.
    const create = page.getByTestId("server-create");
    await moveToLocator(page, create);
    await demoPause(config, "short");
    await demoClick(page, create, config);

    // Wait for the modal to close, then for the Survival card.
    await waitForGone(page.getByTestId("server-name"), "server creation", 30_000);
  });

  const card = page.locator(
    `[data-testid="server-card"][data-server-name="${DEMO_SERVER_NAME}"]`,
  );

  await step("server visible", async () => {
    await assertVisible(card, "Survival server card", 30_000);
    const body = await readBodyText(page);
    if (!body.includes(DEMO_SERVER_NAME)) {
      throw new Error(`server "${DEMO_SERVER_NAME}" did not appear on the dashboard`);
    }
    const cardText = (await card.first().textContent().catch(() => "")) ?? "";
    for (const expected of ["paper", "Java", "25565"]) {
      if (!cardText.toLowerCase().includes(expected.toLowerCase())) {
        console.log(`[demo] note: server card does not mention "${expected}" yet`);
      }
    }
    await demoPause(config, "reveal");
  });

  await step("opening Survival", async () => {
    const openServer = card.getByRole("button", { name: DEMO_SERVER_NAME, exact: true });
    await moveToLocator(page, openServer);
    await demoPause(config, "short");
    await demoClick(page, openServer, config);
    await assertVisible(page.getByTestId("server-detail"), "server detail", 15_000);
  });

  await step("starting server", async () => {
    const start = page.getByTestId("server-start");
    await assertVisible(start, "server Start button", 15_000);
    await demoClick(page, start, config);
    // Prove mcpanel accepted the action and the lifecycle began — without
    // waiting for the full (network-dependent) Minecraft provisioning.
    await waitForServerLifecycle(page);
    // Final frame: hold the begun lifecycle so the recording lands.
    await demoPause(config, "reveal");
    await pause(1000);
  });
}

async function main(): Promise<void> {
  const config = loadDemoConfig();
  console.log(`[demo] target: ${config.baseUrl}`);

  let handle: DemoBrowser | null = null;
  try {
    handle = await launchDemoBrowser(config);
    await runFirstRun(handle.page, handle.context, config);
    const videoPath = await saveRecording(handle);
    if (videoPath) {
      console.log(`[demo] recording saved: ${videoPath}`);
    }
    console.log("[demo] success");
  } finally {
    await closeDemoBrowser(handle ?? {});
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
