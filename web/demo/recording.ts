import { mkdir, rm, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { DemoBrowser } from "./browser.js";

export const VIDEO_WIDTH = 1280;
export const VIDEO_HEIGHT = 720;

export const RECORDING_FILE_NAME = "first-run.webm";

/** `web/artifacts/demos/` — gitignored (see root `.gitignore`). */
export function recordingDir(): string {
  const here = dirname(fileURLToPath(import.meta.url));
  return resolve(here, "../artifacts/demos");
}

/**
 * Finalize the recording and save it as `artifacts/demos/first-run.webm`.
 *
 * Playwright finalizes the `.webm` when the page closes; `saveAs` then
 * moves it to its stable published path. Call this after the workflow,
 * before closing the context/browser.
 */
export async function saveRecording(
  handle: DemoBrowser,
  fileName = RECORDING_FILE_NAME,
): Promise<string | null> {
  const video = handle.page.video();
  if (!video) return null;
  await handle.page.close().catch(() => {});
  const dir = recordingDir();
  await mkdir(dir, { recursive: true });
  const target = resolve(dir, fileName);
  const original = await video.path().catch(() => null);
  await video.saveAs(target);
  // `saveAs` may leave the original behind — keep only the published file.
  if (original && resolve(original) !== target) {
    await rm(original, { force: true }).catch(() => {});
  }
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
 * chapters locate each segment (startClicked / preparing / online / end) so
 * the footage can be trimmed afterwards. Times are video-relative with the
 * page-creation clock as t=0.
 */
export class ChapterLog {
  private readonly events: ChapterEvent[] = [];

  constructor(private readonly startedAt: number) {}

  mark(name: string): number {
    const at = Math.max(0, (Date.now() - this.startedAt) / 1000);
    this.events.push({ name, at });
    console.log(`[demo] chapter ${name} at ${at.toFixed(1)}s`);
    return at;
  }

  at(name: string): number | null {
    return this.events.find((e) => e.name === name)?.at ?? null;
  }

  async save(fileName = "first-run.chapters.json"): Promise<string> {
    const dir = recordingDir();
    await mkdir(dir, { recursive: true });
    const target = resolve(dir, fileName);
    await writeFile(target, JSON.stringify({ events: this.events }, null, 2));
    return target;
  }
}
