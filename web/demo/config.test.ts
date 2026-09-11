import { describe, expect, it } from "vitest";
import { TIMEOUTS, parsePositiveInt } from "./config.js";

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
});
