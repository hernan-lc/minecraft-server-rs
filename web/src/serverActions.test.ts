import { describe, expect, it } from "vitest";
import { serverActionCapabilities } from "./serverActions";

describe("server power action matrix", () => {
  it.each([
    ["offline", { start: true, cancel: false, stop: false, restart: false, kill: false }],
    ["crashed", { start: true, cancel: false, stop: false, restart: false, kill: false }],
    ["preparing", { start: false, cancel: true, stop: false, restart: false, kill: false }],
    ["starting", { start: false, cancel: false, stop: true, restart: false, kill: true }],
    ["online", { start: false, cancel: false, stop: true, restart: true, kill: true }],
    ["stopping", { start: false, cancel: false, stop: false, restart: false, kill: true }],
  ] as const)("matches the lifecycle capabilities for %s", (status, expected) => {
    expect(serverActionCapabilities(status)).toEqual(expected);
  });
});
