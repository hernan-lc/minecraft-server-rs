import { mkdir, rename, rm, stat, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "playwright";
import type { DemoBrowser } from "./browser.js";

export const VIDEO_WIDTH = 1280;
export const VIDEO_HEIGHT = 720;

export const RECORDING_FILE_NAME = "first-run.webm";

/** `web/artifacts/demos/` — gitignored (see root `.gitignore`). */
export function recordingDir(): string {
  const here = dirname(fileURLToPath(import.meta.url));
  return resolve(here, "../artifacts/demos");
}

export function recordingPath(fileName: string): string {
  return resolve(recordingDir(), fileName);
}

export function createRecordingStagingPath(
  stem = "first-run",
): string {
  const safeStem = stem.replace(/[^a-zA-Z0-9_-]/g, "-");
  return resolve(
    recordingDir(),
    `.${safeStem}.${process.pid}.${Date.now()}.webm`,
  );
}

function isNotFoundError(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    (error as { code?: unknown }).code === "ENOENT"
  );
}

/**
 * Stop the Playwright screencast, validate its staging file, and publish it
 * atomically as `artifacts/demos/first-run.webm`.
 */
export async function saveRecording(
  handle: DemoBrowser,
  fileName = RECORDING_FILE_NAME,
): Promise<string> {
  if (handle.recording.active) {
    let stopped = false;
    try {
      await handle.page.screencast.stop();
      stopped = true;
    } finally {
      if (stopped) handle.recording.active = false;
    }
  }

  const source = handle.recording.stagingPath;
  const dir = recordingDir();
  await mkdir(dir, { recursive: true });
  const target = recordingPath(fileName);
  const backup = resolve(
    dir,
    `.${fileName}.${process.pid}.${Date.now()}.bak`,
  );
  let backupCreated = false;

  try {
    const saved = await stat(source);
    if (!saved.isFile() || saved.size === 0) {
      throw new Error("Playwright produced an empty screencast.");
    }

    // Validate the staging file before touching the stable name. Windows does
    // not replace an existing file with rename(), so move the old stable file
    // aside and restore it if publishing the new one fails.
    try {
      await rename(target, backup);
      backupCreated = true;
    } catch (error) {
      if (!isNotFoundError(error)) throw error;
    }
    try {
      await rename(source, target);
    } catch (error) {
      if (backupCreated) {
        await rm(target, { force: true }).catch(() => {});
        await rename(backup, target).catch(() => {});
        backupCreated = false;
      }
      throw error;
    }
    if (backupCreated) {
      await rm(backup, { force: true }).catch(() => {});
      backupCreated = false;
    }

  } catch (error) {
    await rm(source, { force: true }).catch(() => {});
    if (backupCreated) {
      await rm(target, { force: true }).catch(() => {});
      await rename(backup, target).catch(() => {});
    }
    throw error;
  }
  return target;
}

/** Save a full-page failure image without changing the original workflow error. */
export async function saveFailureScreenshot(
  page: Page,
  fileName = "first-run-failed.png",
): Promise<string> {
  const target = recordingPath(fileName);
  await mkdir(recordingDir(), { recursive: true });
  await page.screenshot({ path: target, fullPage: true });
  return target;
}

/** Save sanitized run diagnostics independently of video/chapter finalization. */
export async function saveDiagnosticsArtifact(
  diagnostics: unknown,
  fileName: string,
): Promise<string> {
  const dir = recordingDir();
  await mkdir(dir, { recursive: true });
  const target = recordingPath(fileName);
  await writeFile(target, JSON.stringify(diagnostics, null, 2), "utf8");
  return target;
}

export interface ChapterEvent {
  name: string;
  /** Seconds since the recording started (video-relative). */
  at: number;
}

/**
 * Chapter markers for manual editing.
 *
 * Long phases (Java/core download, first boot) are recorded raw and uncut;
 * chapters locate each workflow segment (setup / login / serverCreated /
 * startClicked / observed lifecycle states / online / end or failed) so the
 * footage can be trimmed afterwards. Times are video-relative with the
 * page-creation clock as t=0.
 */
export class ChapterLog {
  private readonly entries: ChapterEvent[] = [];

  constructor(private readonly startedAt: number) {}

  get events(): readonly ChapterEvent[] {
    return this.entries;
  }

  mark(name: string): number {
    const at = Math.max(0, (Date.now() - this.startedAt) / 1000);
    this.entries.push({ name, at });
    console.log(`[demo] chapter ${name} at ${at.toFixed(1)}s`);
    return at;
  }

  markOnce(name: string): number {
    return this.entries.find((event) => event.name === name)?.at ?? this.mark(name);
  }

  at(name: string): number | null {
    return this.entries.find((e) => e.name === name)?.at ?? null;
  }

  async save(fileName = "first-run.chapters.json"): Promise<string> {
    const dir = recordingDir();
    await mkdir(dir, { recursive: true });
    const target = resolve(dir, fileName);
    await writeFile(target, JSON.stringify({ events: this.entries }, null, 2));
    return target;
  }
}
