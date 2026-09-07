import { render, screen, waitFor } from "@testing-library/preact";
import { describe, expect, it, vi } from "vitest";
import { I18nProvider } from "../i18n";
import { Setup } from "./Setup";

const apiMock = vi.hoisted(() => ({
  setupStatus: vi.fn(),
  setup: vi.fn(),
}));

vi.mock("../api", () => ({ api: apiMock }));

function renderSetup() {
  render(
    <I18nProvider>
      <Setup onDone={vi.fn()} />
    </I18nProvider>,
  );
}

describe("Setup first-run page", () => {
  it("offers the in-page language picker while loading", () => {
    apiMock.setupStatus.mockReturnValue(new Promise(() => {}));
    renderSetup();
    expect(screen.getAllByLabelText("Language").length).toBeGreaterThanOrEqual(1);
  });

  it("offers the in-page language picker on the create-admin form", async () => {
    apiMock.setupStatus.mockResolvedValue({ needs_setup: true });
    renderSetup();
    await waitFor(() => expect(screen.getByText("MCP Panel Setup")).toBeTruthy());
    expect(screen.getAllByLabelText("Language").length).toBeGreaterThanOrEqual(1);
  });
});
