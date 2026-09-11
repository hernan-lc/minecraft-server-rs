import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { loadDemoConfig, TIMEOUTS, parsePositiveInt } from "./config.js";

describe("demo timeout configuration", () => {
  it("accepts positive finite integer values", () => {
    expect(parsePositiveInt("1200000", TIMEOUTS.online)).toBe(1_200_000);
    expect(parsePositiveInt("12.9", TIMEOUTS.ui)).toBe(12);
  });

  it.each([undefined, "", "0", "-1", "NaN", "Infinity"]) (
    "rejects %s and uses the fallback",
    (value) => {
      expect(parsePositiveInt(value, 30_000)).toBe(30_000);
    },
  );

  it("keeps process environment ahead of .env.demo and CLI ahead of both", () => {
    const directory = mkdtempSync(join(tmpdir(), "mcpanel-demo-config-"));
    const envPath = resolve(directory, ".env.demo");
    writeFileSync(
      envPath,
      [
        "MCPANEL_DEMO_URL=http://from-file:8080",
        "MCPANEL_DEMO_CATALOG_TIMEOUT_MS=91000",
      ].join("\n"),
    );

    const previousArgv = process.argv;
    const previousUrl = process.env.MCPANEL_DEMO_URL;
    const previousCatalogTimeout = process.env.MCPANEL_DEMO_CATALOG_TIMEOUT_MS;
    try {
      process.env.MCPANEL_DEMO_URL = "http://from-process:8080";
      delete process.env.MCPANEL_DEMO_CATALOG_TIMEOUT_MS;
      process.argv = previousArgv.slice(0, 2);

      const processConfig = loadDemoConfig([envPath]);
      expect(processConfig.envFile).toBe(envPath);
      expect(processConfig.baseUrl).toBe("http://from-process:8080");
      expect(processConfig.catalogTimeoutMs).toBe(91_000);

      process.argv = [...previousArgv.slice(0, 2), "--url", "http://from-cli:8080"];
      const cliConfig = loadDemoConfig([envPath]);
      expect(cliConfig.baseUrl).toBe("http://from-cli:8080");
    } finally {
      process.argv = previousArgv;
      if (previousUrl === undefined) delete process.env.MCPANEL_DEMO_URL;
      else process.env.MCPANEL_DEMO_URL = previousUrl;
      if (previousCatalogTimeout === undefined) delete process.env.MCPANEL_DEMO_CATALOG_TIMEOUT_MS;
      else process.env.MCPANEL_DEMO_CATALOG_TIMEOUT_MS = previousCatalogTimeout;
      rmSync(directory, { recursive: true, force: true });
    }
  });
});
