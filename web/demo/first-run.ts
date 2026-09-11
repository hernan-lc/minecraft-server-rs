import "dotenv/config";
import type { Page } from "playwright";
import { closeDemoBrowser, launchDemoBrowser, type DemoBrowser } from "./browser.js";
import { loadDemoConfig, type DemoConfig } from "./config.js";
import {
  assertVisible,
  demoPause,
  readBodyText,
  step,
  waitForGone,
  waitForSelectOptions,
} from "./helpers.js";
import { saveRecording } from "./recording.js";

const DEMO_SERVER_NAME = "Survival";

/**
 * First Run → Create Server, recorded to `artifacts/demos/first-run.webm`.
 *
 * 1. Open /setup from a fresh data directory.
 * 2. Create the initial admin account.
 * 3. Log in and reach the dashboard.
 * 4. Open New Server, create a Paper server using the latest available
 *    Minecraft version, verify the application's Java selection, accept the
 *    EULA, and submit.
 * 5. Verify the Survival card appears; leave the dashboard visible.
 *
 * Everything is deterministic (`data-testid` locators). The password is
 * filled directly and never logged.
 */
async function runFirstRun(page: Page, config: DemoConfig): Promise<void> {
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
    await page.getByTestId("setup-username").fill(config.username);
    await demoPause(config, "short");
    await page.getByTestId("setup-password").fill(config.password);
    await page.getByTestId("setup-password-confirm").fill(config.password);
    await demoPause(config, "short");
    await page.getByTestId("setup-submit").click();
    // Setup redirects to / (login). Wait for the login form, not a sleep.
    await assertVisible(page.getByTestId("login-username"), "login form after setup", 15_000);
  });

  await step("logging in", async () => {
    await page.getByTestId("login-username").fill(config.username);
    await demoPause(config, "short");
    await page.getByTestId("login-password").fill(config.password);
    await demoPause(config, "short");
    await page.getByTestId("login-submit").click();
    await assertVisible(page.getByTestId("new-server"), "dashboard", 15_000);
  });

  await step("opening new server", async () => {
    await page.getByTestId("new-server").click();
    await assertVisible(page.getByTestId("server-name"), "new server form", 15_000);
  });

  await step(`creating ${DEMO_SERVER_NAME}`, async () => {
    await page.getByTestId("server-name").fill(DEMO_SERVER_NAME);
    await demoPause(config, "short");

    // Core: paper.
    await page.getByTestId("server-core").selectOption("paper");

    // Minecraft version: use the latest provided by the actual UI/API —
    // never a hard-coded version. The versions list populates async.
    const versions = await waitForSelectOptions(page, "server-version", "Minecraft versions", 30_000);
    const minecraftVersion = await page.getByTestId("server-version").inputValue();
    if (!minecraftVersion) {
      throw new Error("Minecraft versions failed to populate");
    }
    console.log(`[demo] minecraft: ${minecraftVersion} (latest of ${versions.length})`);

    // Java: verify the application's selection rather than overriding it.
    // Current Minecraft 26.x releases expect Java 25; silently flipping
    // Java 21 → 25 here would hide an application regression.
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

    await demoPause(config, "short");

    // EULA must be accepted or the server refuses to start.
    await page.getByTestId("server-eula").check();

    await demoPause(config, "short");
    await page.getByTestId("server-create").click();

    // Wait for the modal to close, then for the Survival card.
    await waitForGone(page.getByTestId("server-name"), "server creation", 30_000);
  });

  await step("server visible", async () => {
    const card = page.getByTestId("server-card");
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
  });

  // Final frame for the recording: hold the stable dashboard before cleanup.
  if (config.slow) {
    await new Promise((resolve) => setTimeout(resolve, 1500));
  }
}

async function main(): Promise<void> {
  const config = loadDemoConfig();
  console.log(`[demo] target: ${config.baseUrl}`);

  let handle: DemoBrowser | null = null;
  try {
    handle = await launchDemoBrowser(config);
    await runFirstRun(handle.page, config);
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
