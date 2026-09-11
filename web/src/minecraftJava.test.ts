import { describe, expect, it } from "vitest";
import { recommendedJavaForVersion } from "./minecraftJava";

describe("recommendedJavaForVersion", () => {
  it("selects Java 25 for Minecraft 26.x releases", () => {
    expect(recommendedJavaForVersion("26.2")).toBe(25);
    expect(recommendedJavaForVersion("26.1.2")).toBe(25);
    expect(recommendedJavaForVersion("26.3-snapshot-9")).toBe(25);
  });

  it("keeps Java 21 for the 1.21 era", () => {
    expect(recommendedJavaForVersion("1.21.8")).toBe(21);
    expect(recommendedJavaForVersion("1.21")).toBe(21);
  });

  it("falls back to Java 21 for empty or unparseable versions", () => {
    expect(recommendedJavaForVersion("")).toBe(21);
    expect(recommendedJavaForVersion("weird")).toBe(21);
  });
});
