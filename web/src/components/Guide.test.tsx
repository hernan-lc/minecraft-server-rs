import { fireEvent, render, screen, waitFor } from "@testing-library/preact";
import { describe, expect, it, vi } from "vitest";
import { I18nProvider } from "../i18n";
import { Guide, computePopoverLayout } from "./Guide";

/** jsdom has no layout; give anchors a real box so the tour can find them. */
function box(element: HTMLElement, rect: { top: number; left: number; width: number; height: number }) {
  element.getBoundingClientRect = () =>
    ({ ...rect, right: rect.left + rect.width, bottom: rect.top + rect.height, x: rect.left, y: rect.top, toJSON() {} }) as DOMRect;
}

describe("guided tour", () => {
  it("opens once per key, remembers dismissal and can be replayed", async () => {
    render(
      <I18nProvider>
        <Guide storageKey="test-user" steps={[{ title: "First", body: "Details" }]} />
      </I18nProvider>,
    );
    expect(await screen.findByRole("dialog", { name: "First" })).toBeInTheDocument();
    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(localStorage.getItem("mcpanel:guide:v2:test-user")).toBe("seen");

    fireEvent.click(screen.getByRole("button", { name: "Guided tour" }));
    expect(await screen.findByRole("dialog", { name: "First" })).toBeInTheDocument();
  });

  it("does not open automatically once seen", () => {
    localStorage.setItem("mcpanel:guide:v2:seen-user", "seen");
    render(
      <I18nProvider>
        <Guide storageKey="seen-user" steps={[{ title: "First", body: "Details" }]} />
      </I18nProvider>,
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("runs enter hooks, navigates with buttons and arrow keys, and finishes", async () => {
    const enter = vi.fn();
    render(
      <I18nProvider>
        <Guide
          steps={[
            { title: "First", body: "One" },
            { title: "Second", body: "Two", enter },
            { title: "Third", body: "Three" },
          ]}
        />
      </I18nProvider>,
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Guided tour" }));
    expect(screen.queryByRole("button", { name: "Previous" })).not.toBeInTheDocument();
    expect(screen.getByText("Step 1 of 3")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByRole("dialog", { name: "Second" })).toBeInTheDocument();
    await waitFor(() => expect(enter).toHaveBeenCalledTimes(1));

    fireEvent.keyDown(document, { key: "ArrowRight" });
    expect(await screen.findByRole("dialog", { name: "Third" })).toBeInTheDocument();
    fireEvent.keyDown(document, { key: "ArrowLeft" });
    expect(await screen.findByRole("dialog", { name: "Second" })).toBeInTheDocument();
    fireEvent.keyDown(document, { key: "ArrowRight" });

    fireEvent.click(await screen.findByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("spotlights the target element", async () => {
    const target = document.createElement("div");
    target.setAttribute("data-tour", "thing");
    box(target, { top: 100, left: 50, width: 200, height: 40 });
    document.body.appendChild(target);
    target.scrollIntoView = vi.fn();

    render(
      <I18nProvider>
        <Guide steps={[{ title: "Thing", body: "Look here", target: '[data-tour="thing"]' }]} />
      </I18nProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Guided tour" }));
    await screen.findByRole("dialog", { name: "Thing" });
    await waitFor(() => {
      const ring = document.querySelector<HTMLElement>('[data-testid="guide-tour"] .ring-2');
      expect(ring).not.toBeNull();
      expect(ring!.style.top).toBe("92px");
      expect(ring!.style.left).toBe("42px");
      expect(ring!.style.width).toBe("216px");
    });
    target.remove();
  });

  it("skips optional steps whose target never appears", async () => {
    render(
      <I18nProvider>
        <Guide
          steps={[
            { title: "First", body: "One" },
            { title: "Missing", body: "Nope", target: '[data-tour="absent"]', optional: true },
            { title: "Last", body: "End" },
          ]}
        />
      </I18nProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Guided tour" }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByRole("dialog", { name: "Last" }, { timeout: 3000 })).toBeInTheDocument();
  });
});

describe("popover placement", () => {
  const viewport = { width: 1000, height: 800 };
  const popover = { width: 300, height: 150 };

  it("centres the popover without a target", () => {
    expect(computePopoverLayout(null, popover, viewport)).toMatchObject({ side: "center", top: 325, left: 350 });
  });

  it("prefers below the target and flips above when there is no room", () => {
    expect(computePopoverLayout({ top: 100, left: 400, width: 200, height: 40 }, popover, viewport).side).toBe("bottom");
    expect(computePopoverLayout({ top: 700, left: 400, width: 200, height: 40 }, popover, viewport).side).toBe("top");
  });

  it("keeps the popover inside the viewport horizontally", () => {
    const layout = computePopoverLayout({ top: 100, left: 950, width: 40, height: 40 }, popover, viewport);
    expect(layout.left + popover.width).toBeLessThanOrEqual(viewport.width - 12);
  });

  it("floats over the page for targets larger than the viewport", () => {
    const layout = computePopoverLayout({ top: 0, left: 0, width: 1000, height: 800 }, popover, viewport);
    expect(layout.side).toBe("center");
    expect(layout.top).toBeGreaterThanOrEqual(12);
  });
});
