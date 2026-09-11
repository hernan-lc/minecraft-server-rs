import { config as loadDotenv } from "dotenv";
import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export interface DemoConfig {
  baseUrl: string;
  username: string;
  password: string;
  headless: boolean;
  slow: boolean;
  /** Cold validation deliberately avoids relying on a demo prewarm step. */
  cold: boolean;
  uiTimeoutMs: number;
  catalogTimeoutMs: number;
  createServerTimeoutMs: number;
  startTimeoutMs: number;
  onlineTimeoutMs: number;
}

export const TIMEOUTS = {
  ui: 30_000,
  navigation: 30_000,
  catalog: 90_000,
  createServer: 60_000,
  startAccepted: 60_000,
  online: 20 * 60_000,
} as const;

/**
 * Disposable demo credentials. The demo creates this account itself on a
 * fresh data directory, so no setup is required to run it.
 */
const DEFAULT_PASSWORD = "mcpanel-demo-password";

function loadDemoEnvFiles(): void {
  // Optional overrides only — the demo runs with zero configuration.
  // `tsx demo/first-run.ts` runs with `web/` as cwd; also resolve relative
  // to this file so it works from the repo root.
  const here = dirname(fileURLToPath(import.meta.url));
  for (const path of [
    resolve(here, "../.env.demo"),
    resolve(here, ".env.demo"),
    resolve(process.cwd(), ".env.demo"),
  ]) {
    if (existsSync(path)) {
      loadDotenv({ path, override: false });
      return;
    }
  }
  loadDotenv({ override: false });
}

function parseBoolean(value: string | undefined, fallback: boolean): boolean {
  if (value === undefined) return fallback;
  return value.trim().toLowerCase() === "true";
}

/** Parse a positive integer environment override without accepting NaN/zero. */
export function parsePositiveInt(value: string | undefined, fallback: number): number {
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed > 0 ? Math.floor(parsed) : fallback;
}

function cliFlag(name: string): boolean {
  return process.argv.includes(`--${name}`);
}

function cliValue(name: string): string | null {
  const index = process.argv.indexOf(`--${name}`);
  if (index === -1) return null;
  const value = process.argv[index + 1];
  return value && !value.startsWith("--") ? value : null;
}

/**
 * Zero-config demo configuration.
 *
 * Presentation pacing is the default: `npm run demo:first-run` produces a
 * human-readable video without flags. `--fast` is the special mode for
 * development/debugging.
 *
 * Precedence: CLI flags > environment variables > built-in defaults.
 */
export function loadDemoConfig(): DemoConfig {
  loadDemoEnvFiles();

  const headed = cliFlag("headed") || cliFlag("headful");
  const headless =
    cliFlag("headless") || (!headed && parseBoolean(process.env.MCPANEL_DEMO_HEADLESS, true));
  const slow =
    !cliFlag("fast") && (cliFlag("slow") || parseBoolean(process.env.MCPANEL_DEMO_SLOW, true));
  const cold =
    cliFlag("cold") || (!cliFlag("warm") && parseBoolean(process.env.MCPANEL_DEMO_COLD, false));

  const uiTimeoutMs = parsePositiveInt(
    process.env.MCPANEL_DEMO_UI_TIMEOUT_MS,
    TIMEOUTS.ui,
  );
  const catalogTimeoutMs = parsePositiveInt(
    process.env.MCPANEL_DEMO_CATALOG_TIMEOUT_MS,
    TIMEOUTS.catalog,
  );
  const createServerTimeoutMs = parsePositiveInt(
    process.env.MCPANEL_DEMO_CREATE_SERVER_TIMEOUT_MS,
    TIMEOUTS.createServer,
  );
  const startTimeoutMs = parsePositiveInt(
    process.env.MCPANEL_DEMO_START_TIMEOUT_MS,
    TIMEOUTS.startAccepted,
  );
  const onlineTimeoutMs = parsePositiveInt(
    process.env.MCPANEL_DEMO_ONLINE_TIMEOUT_MS,
    TIMEOUTS.online,
  );

  const baseUrl = (
    cliValue("url") ??
    process.env.MCPANEL_DEMO_URL ??
    "http://127.0.0.1:8080"
  ).trim().replace(/\/$/, "");
  const username = (
    cliValue("user") ??
    process.env.MCPANEL_DEMO_USERNAME ??
    "admin"
  ).trim();
  const password =
    cliValue("password") ?? process.env.MCPANEL_DEMO_PASSWORD ?? DEFAULT_PASSWORD;

  if (!password) {
    throw new Error("Demo password must not be empty.");
  }

  return {
    baseUrl,
    username,
    password,
    headless,
    slow,
    cold,
    uiTimeoutMs,
    catalogTimeoutMs,
    createServerTimeoutMs,
    startTimeoutMs,
    onlineTimeoutMs,
  };
}
