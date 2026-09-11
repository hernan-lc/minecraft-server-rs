import { describe, expect, it, vi } from "vitest";
import { ChapterLog } from "./recording.js";

describe("ChapterLog", () => {
  it("records a failed marker for failed raw footage", () => {
    const log = new ChapterLog(Date.now());

    log.mark("failed");

    expect(log.events).toEqual([{ name: "failed", at: expect.any(Number) }]);
    expect(log.at("failed")).not.toBeNull();
  });

  it("does not duplicate observed lifecycle markers", () => {
    const consoleSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    const log = new ChapterLog(Date.now());

    log.markOnce("starting");
    log.markOnce("starting");

    expect(log.events).toHaveLength(1);
    consoleSpy.mockRestore();
  });
});
