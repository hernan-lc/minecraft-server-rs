import { render, screen } from "@testing-library/preact";
import { describe, expect, it } from "vitest";
import { I18nProvider } from "../i18n";
import { ServerActivity } from "./ServerActivity";

describe("ServerActivity", () => {
  it("renders preparation progress exactly once", () => {
    render(
      <I18nProvider>
        <ServerActivity
          status="preparing"
          progress={{ stage: "Downloading Paper 26.2", fraction: 0.38 }}
        />
      </I18nProvider>,
    );

    const activity = screen.getByTestId("server-activity");
    expect(activity).toBeInTheDocument();
    expect(activity).toHaveAttribute("data-stage", "Downloading Paper 26.2");
    expect(activity).toHaveAttribute("data-progress-stage", "Downloading Paper 26.2");
    expect(activity).toHaveAttribute("data-fraction", "0.38");
    expect(screen.getAllByText("Downloading Paper 26.2")).toHaveLength(1);
    expect(screen.getAllByText("38%")).toHaveLength(1);
  });

  it("removes the progress surface after preparation", () => {
    const { rerender } = render(
      <I18nProvider>
        <ServerActivity
          status="preparing"
          progress={{ stage: "Downloading Paper 26.2", fraction: 0.38 }}
        />
      </I18nProvider>,
    );

    rerender(
      <I18nProvider>
        <ServerActivity status="starting" progress={null} />
      </I18nProvider>,
    );

    expect(screen.queryByText("Downloading Paper 26.2")).not.toBeInTheDocument();
    expect(screen.getByText("Starting Minecraft server…")).toBeInTheDocument();
  });
});
