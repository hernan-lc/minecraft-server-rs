import { render, screen } from "@testing-library/preact";
import { describe, expect, it } from "vitest";
import { Button } from "./ui";

function classesOf(name: string): string[] {
  const button = screen.getByRole("button", { name });
  return (button.getAttribute("class") ?? "").split(/\s+/);
}

describe("Button padding", () => {
  it("square buttons omit the base padding so no override can lose the cascade", () => {
    // Tailwind emits `px-0` before `px-4`, so with equal specificity the base
    // padding always won and crushed icons in fixed-size buttons into dots.
    render(
      <Button square aria-label="Start">
        <span class="hidden sm:inline">Start</span>
      </Button>,
    );
    const classes = classesOf("Start");
    expect(classes).toContain("p-0");
    expect(classes).not.toContain("px-4");
    expect(classes).not.toContain("py-2");
  });

  it("regular buttons keep their base padding", () => {
    render(<Button>Start</Button>);
    const classes = classesOf("Start");
    expect(classes).toContain("px-4");
    expect(classes).toContain("py-2");
    expect(classes).not.toContain("p-0");
  });
});
