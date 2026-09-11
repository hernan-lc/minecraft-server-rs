import "dotenv/config";
import type { WriteStream } from "node:fs";
import type { BrowserContext, Page } from "playwright";
import { closeDemoBrowser, disableRequestInterception, launchDemoBrowser, type DemoBrowser } from "./browser.js";
import { loadDemoConfig, type DemoConfig } from "./config.js";
import { recommendedJavaForVersion } from "../src/minecraftJava.js";
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
  readServerDiagnostics,
  step,
  waitForGone,
  waitForSelectOptions,
  waitForSelectValue,
  waitForServerOnline,
  waitForServerLifecycle,
} from "./helpers.js";
import {
  ChapterLog,
  saveFailureScreenshot,
  saveRecording,
} from "./recording.js";

const DEMO_SERVER_NAME = "Survival";

/**
 * First Run → Create Server → Start → Online, recorded to
 * `artifacts/demos/first-run.webm`.
 *
 * The story, without narration: configure mcpanel → sign in → create a
 * Paper server → open it → start it → watch it come online for real
 * (Java download, core download, boot). The full provisioning is recorded
 * raw and uncut for manual editing afterwards.
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
  chapters: ChapterLog,
): Promise<void> {
  await step("opening setup", async () => {
    await page.goto(`${config.baseUrl}/setup`);
    // The setup form is the proof that a fresh data directory is in use.
    // A completed setup renders an "already completed" notice instead.
    try {
      await assertVisible(page.getByTestId("setup-username"), "setup form", config.uiTimeoutMs);
      chapters.mark("setup");
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
    await assertVisible(
      page.getByTestId("login-username"),
      "login form after setup",
      config.uiTimeoutMs,
    );
  });

  await step("logging in", async () => {
    // The username defaults to the admin name; only the password is entered.
    await demoFocus(page, page.getByTestId("login-password"), config);
    await page.getByTestId("login-password").fill(config.password);
    await demoClick(page, page.getByTestId("login-submit"), config);
    await assertVisible(page.getByTestId("new-server"), "dashboard", config.uiTimeoutMs);
    chapters.mark("login");
    chapters.mark("dashboard");
    // Last full navigation is done (the rest is SPA state): drop request
    // interception so live traffic runs at full speed. Cursor styles are
    // already applied to every loaded document.
    await disableRequestInterception(context);
  });

  await step("opening new server", async () => {
    await demoClick(page, page.getByTestId("new-server"), config);
    await assertVisible(page.getByTestId("server-name"), "new server form", config.uiTimeoutMs);
    chapters.mark("newServer");
  });

  await step(`creating ${DEMO_SERVER_NAME}`, async () => {
    await demoType(page, page.getByTestId("server-name"), DEMO_SERVER_NAME, config);

    // Core: paper. Native selects are shown, not dropdown-automated.
    await waitForSelectOptions(
      page,
      "server-core",
      "server providers",
      config.catalogTimeoutMs,
    );
    await demoSelectOption(page, page.getByTestId("server-core"), "paper", config);

    // Minecraft version: use the latest provided by the actual UI/API —
    // never a hard-coded version. The versions list populates async.
    const versions = await waitForSelectOptions(
      page,
      "server-version",
      "Minecraft versions",
      config.catalogTimeoutMs,
    );
    const latestVersion = versions[0];
    if (!latestVersion) {
      throw new Error("Minecraft versions failed to populate");
    }
    await waitForSelectValue(
      page,
      "server-version",
      latestVersion,
      "latest Minecraft version",
      config.catalogTimeoutMs,
    );
    await moveToLocator(page, page.getByTestId("server-version"));
    await demoPause(config, "short");
    const minecraftVersion = await page.getByTestId("server-version").inputValue();
    if (!minecraftVersion) {
      throw new Error("Minecraft versions failed to populate");
    }
    if (minecraftVersion !== latestVersion) {
      throw new Error(
        `expected the latest Minecraft version ${latestVersion}, but the form selected ${minecraftVersion}`,
      );
    }
    console.log(`[demo] minecraft: ${minecraftVersion} (latest of ${versions.length})`);

    // Java: verify the application's selection rather than overriding it.
    // Verify the application's compatibility mapping rather than overriding
    // it. Silently flipping Java here would hide an application regression.
    const expectedJava = String(recommendedJavaForVersion(minecraftVersion));
    await moveToLocator(page, page.getByTestId("server-java"));
    await demoPause(config, "short");
    await waitForSelectValue(
      page,
      "server-java",
      expectedJava,
      "compatible Java version",
      config.uiTimeoutMs,
    );
    const java = await page.getByTestId("server-java").inputValue();
    console.log(`[demo] java: ${java}`);
    if (java !== expectedJava) {
      throw new Error(
        `expected Java version ${expectedJava} for Minecraft ${minecraftVersion}, but the application selected Java ${java || "(none)"}`,
      );
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
    await waitForGone(
      page.getByTestId("server-name"),
      "server creation",
      config.createServerTimeoutMs,
    );
  });

  const card = page.locator(
    `[data-testid="server-card"][data-server-name="${DEMO_SERVER_NAME}"]`,
  );

  await step("server visible", async () => {
    await assertVisible(card, "Survival server card", config.uiTimeoutMs);
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
    chapters.mark("serverCreated");
    await demoPause(config, "reveal");
  });

  await step("opening Survival", async () => {
    const openServer = card.getByRole("button", { name: DEMO_SERVER_NAME, exact: true });
    await moveToLocator(page, openServer);
    await demoPause(config, "short");
    await demoClick(page, openServer, config);
    await assertVisible(page.getByTestId("server-detail"), "server detail", config.uiTimeoutMs);
    chapters.mark("serverOpened");
  });

  await step("starting server", async () => {
    const start = page.getByTestId("server-start");
    await assertVisible(start, "server Start button", config.uiTimeoutMs);
    await demoClick(page, start, config);
    chapters.mark("startClicked");
    // Checkpoint: mcpanel accepted the action and the lifecycle began. A
    // warm/cached start may skip a visible intermediate state, so online is
    // explicitly accepted here too.
    const acceptedStatus = await waitForServerLifecycle(
      page,
      ["preparing", "starting", "online"],
      config.startTimeoutMs,
    );
    chapters.mark("startAccepted");
    if (acceptedStatus === "preparing" || acceptedStatus === "starting") {
      chapters.markOnce(acceptedStatus);
    } else if (acceptedStatus === "online") {
      chapters.markOnce("online");
    }
    // Let the accepted lifecycle state settle visibly before the long raw
    // provisioning section begins. This is pacing, not synchronization.
    await demoPause(config, "reveal");
  });

  await step("awaiting online", async () => {
    // The whole point of the demo: Java download → core download → boot,
    // for real. This takes minutes on first start and is recorded uncut.
    // Heartbeat every 30 s so long provisioning visibly progresses.
    const started = Date.now();
    const logHeartbeat = async (): Promise<void> => {
      const elapsed = Math.round((Date.now() - started) / 1000);
      const diagnostics = await readServerDiagnostics(page).catch(() => null);
      if (!diagnostics) {
        console.log(`[demo] provisioning… ${elapsed}s status=unknown stage=unknown fraction=unknown`);
        return;
      }
      const fraction = diagnostics.fraction === null ? "unknown" : diagnostics.fraction;
      console.log(
        `[demo] provisioning… ${elapsed}s status=${diagnostics.status} stage=${diagnostics.stage ?? "unknown"} fraction=${fraction}`,
      );
    };
    await logHeartbeat();
    const beat = setInterval(() => {
      void logHeartbeat();
    }, 30_000);
    try {
      await waitForServerOnline(page, config.onlineTimeoutMs, (status) => {
        if (status === "preparing" || status === "starting" || status === "online") {
          chapters.markOnce(status);
        }
      });
    } finally {
      clearInterval(beat);
    }
    chapters.markOnce("online");
    console.log("[demo] status: online");
    // Hold the real online state for raw-footage editing. The unconditional
    // 2.5s hold keeps the final frame useful even in --fast mode.
    await demoPause(config, "reveal");
    await pause(2500);
  });
}

async function main(): Promise<void> {
  const config = loadDemoConfig();
  const logIndex = process.argv.indexOf("--log");
  const logFile =
    logIndex !== -1 ? process.argv[logIndex + 1] : undefined;
  let logStream: WriteStream | null = null;
  if (logFile) {
    const { createWriteStream } = await import("node:fs");
    const { mkdir } = await import("node:fs/promises");
    const { dirname } = await import("node:path");
    await mkdir(dirname(logFile), { recursive: true });
    logStream = createWriteStream(logFile, { encoding: "utf8" });
    logStream.on("error", () => {
      logStream = null;
    });
    for (const method of ["log", "error"] as const) {
      const original = console[method].bind(console);
      console[method] = (...args: unknown[]) => {
        original(...args);
        try {
          logStream?.write(
            args
              .map((a) => (typeof a === "string" ? a : JSON.stringify(a)))
              .join(" ") + "\n",
          );
        } catch {
          // Logging must never break the demo.
        }
      };
    }
  }
  console.log(`[demo] target: ${config.baseUrl}`);
  console.log(`[demo] cache mode: ${config.cold ? "cold validation" : "warm recording"}`);

  let handle: DemoBrowser | null = null;
  let chapters: ChapterLog | null = null;
  let workflowError: unknown = null;
  try {
    handle = await launchDemoBrowser(config);
    chapters = new ChapterLog(handle.startedAt);

    try {
      await runFirstRun(handle.page, handle.context, config, chapters);
      chapters.mark("end");
    } catch (error) {
      workflowError = error;
      chapters.mark("failed");
      try {
        const screenshot = await saveFailureScreenshot(handle.page);
        console.log(`[demo] failure screenshot saved: ${screenshot}`);
      } catch (screenshotError) {
        console.error(
          `[demo] could not save failure screenshot: ${
            screenshotError instanceof Error ? screenshotError.message : String(screenshotError)
          }`,
        );
      }
    } finally {
      try {
        const chapterPath = await chapters.save();
        console.log(`[demo] chapters saved: ${chapterPath}`);
      } catch (chapterError) {
        console.error(
          `[demo] could not save chapters: ${
            chapterError instanceof Error ? chapterError.message : String(chapterError)
          }`,
        );
        if (!workflowError) workflowError = chapterError;
      }

      try {
        const fileName = workflowError ? "first-run-failed.webm" : "first-run.webm";
        const videoPath = await saveRecording(handle, fileName);
        if (videoPath) console.log(`[demo] recording saved: ${videoPath}`);
      } catch (recordingError) {
        console.error(
          `[demo] could not save recording: ${
            recordingError instanceof Error ? recordingError.message : String(recordingError)
          }`,
        );
        if (!workflowError) workflowError = recordingError;
      }
    }

    if (workflowError) throw workflowError;
    console.log("[demo] success");
  } finally {
    await closeDemoBrowser(handle ?? {});
    logStream?.end();
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
