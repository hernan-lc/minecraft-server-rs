import { fireEvent, render, screen, waitFor } from "@testing-library/preact";
import { describe, expect, it, beforeEach, vi } from "vitest";
import { I18nProvider } from "../i18n";
import type { ConsoleLine, ServerEvent, ServerSnapshot } from "../types";
import { Console } from "./Console";

const apiMock = vi.hoisted(() => ({ openConsole: vi.fn() }));

vi.mock("../api", () => ({ openConsole: apiMock.openConsole }));

class FakeSocket {
  static instances: FakeSocket[] = [];
  readyState = 1;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;

  constructor() {
    FakeSocket.instances.push(this);
    queueMicrotask(() => this.onopen?.());
  }

  close() {
    this.readyState = 3;
    this.onclose?.();
  }

  emit(message: ServerEvent) {
    this.onmessage?.({ data: JSON.stringify(message) } as MessageEvent);
  }
}

function line(seq: number): ConsoleLine {
  return { seq, stream: "stdout", line: `line-${seq}` };
}

function preparingStatus(): ServerSnapshot {
  return {
    status: "preparing",
    pid: null,
    uptime_secs: null,
    crashes: 0,
    progress: { stage: "downloading paper 26.2", fraction: 0.38 },
  };
}

function renderConsole(onProgress = vi.fn()) {
  const onStatus = vi.fn();
  render(
    <I18nProvider>
      <Console serverId="server-1" onStatus={onStatus} onProgress={onProgress} />
    </I18nProvider>,
  );
  return { onStatus, onProgress };
}

describe("console sequence handling", () => {
  beforeEach(() => {
    FakeSocket.instances = [];
    apiMock.openConsole.mockImplementation(async () => new FakeSocket() as unknown as WebSocket);
  });

  it("deduplicates backfill/live overlap and forwards current progress", async () => {
    const { onProgress } = renderConsole();
    await waitFor(() => expect(FakeSocket.instances).toHaveLength(1));
    const socket = FakeSocket.instances[0];

    socket.emit({
      type: "backfill",
      status: preparingStatus(),
      lines: [line(10), line(11), line(11), line(12)],
      through_seq: 12,
    });
    socket.emit({ type: "console", ...line(12) });
    socket.emit({ type: "console", ...line(13) });

    expect(await screen.findByText("line-13")).toBeInTheDocument();
    expect(screen.getAllByText("line-11")).toHaveLength(1);
    expect(screen.getAllByText("line-12")).toHaveLength(1);
    expect(screen.queryByText("downloading paper 26.2")).not.toBeInTheDocument();
    expect(onProgress).toHaveBeenCalledWith({
      stage: "downloading paper 26.2",
      fraction: 0.38,
    });
  });

  it("does not duplicate history when a reconnect backfill overlaps live lines", async () => {
    vi.useFakeTimers();
    try {
      renderConsole();
      await vi.waitFor(() => expect(FakeSocket.instances).toHaveLength(1));
      const first = FakeSocket.instances[0];
      first.emit({
        type: "backfill",
        status: preparingStatus(),
        lines: [line(100), line(101), line(102)],
        through_seq: 102,
      });
      first.emit({ type: "console", ...line(102) });
      first.emit({ type: "console", ...line(103) });
      expect(await screen.findByText("line-103")).toBeInTheDocument();

      first.close();
      await vi.advanceTimersByTimeAsync(1000);
      await vi.waitFor(() => expect(FakeSocket.instances).toHaveLength(2));
      const second = FakeSocket.instances[1];
      second.emit({
        type: "backfill",
        status: preparingStatus(),
        lines: [line(100), line(101), line(102), line(103)],
        through_seq: 103,
      });
      second.emit({ type: "console", ...line(103) });
      second.emit({ type: "console", ...line(104) });

      expect(await screen.findByText("line-104")).toBeInTheDocument();
      for (const seq of [100, 101, 102, 103, 104]) {
        expect(screen.getAllByText(`line-${seq}`)).toHaveLength(1);
      }
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows only a compact progress label in fullscreen mode", async () => {
    render(
      <I18nProvider>
        <Console
          serverId="server-1"
          status="preparing"
          progress={{ stage: "downloading paper 26.2", fraction: 0.38 }}
          onStatus={vi.fn()}
        />
      </I18nProvider>,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Expand console" }));

    expect(screen.getByText(/Preparing.*38%/)).toBeInTheDocument();
    expect(screen.queryByText("downloading paper 26.2")).not.toBeInTheDocument();
  });
});
