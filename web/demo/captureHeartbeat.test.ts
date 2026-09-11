import { describe, expect, it, vi } from "vitest";
import {
  CAPTURE_HEARTBEAT_INTERVAL_MS,
  CAPTURE_HEARTBEAT_TEST_ID,
  startCaptureHeartbeat,
} from "./captureHeartbeat.js";

describe("capture heartbeat", () => {
  it("installs once and disposes idempotently without using input APIs", async () => {
    const evaluate = vi.fn().mockResolvedValue(undefined);
    const page = { evaluate } as never;

    const heartbeat = await startCaptureHeartbeat(page);
    expect(evaluate).toHaveBeenCalledTimes(1);
    const args = evaluate.mock.calls[0][1] as {
      intervalMs: number;
      testId: string;
    };
    expect(args.intervalMs).toBe(CAPTURE_HEARTBEAT_INTERVAL_MS);
    expect(args.testId).toBe(CAPTURE_HEARTBEAT_TEST_ID);

    await heartbeat.stop();
    await heartbeat.stop();
    expect(evaluate).toHaveBeenCalledTimes(2);
  });
});
