import { fireEvent, render, screen, waitFor } from "@testing-library/preact";
import { describe, expect, it, beforeEach, vi } from "vitest";
import { I18nProvider } from "../i18n";
import type { ConsoleLine, ServerEvent, ServerSnapshot } from "../types";
import {
  Console,
  consoleErrorKind,
  consoleRetryDelay,
  INITIAL_BACKFILL_TIMEOUT_MS,
  isTerminalConsoleError,
} from "./Console";
import { ApiError } from "../api";

const apiMock = vi.hoisted(() => ({ openConsole: vi.fn(), logs: vi.fn() }));

vi.mock("../api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api")>();
  return {
    ...actual,
    api: { ...actual.api, logs: apiMock.logs },
    openConsole: apiMock.openConsole,
  };
});

class FakeSocket {
  static instances: FakeSocket[] = [];
  readyState = 1;
  onopen: (() => void) | null = null;
  onclose: ((event?: { code?: number }) => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;

  constructor() {
    FakeSocket.instances.push(this);
  }

  open() {
    this.onopen?.();
  }

  close(code = 1000) {
    this.readyState = 3;
    this.onclose?.({ code });
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

function renderConsole(status: "preparing" | "starting" | "online" = "preparing", onProgress = vi.fn()) {
  const onStatus = vi.fn();
  render(
    <I18nProvider>
      <Console
        serverId="server-1"
        status={status}
        onStatus={onStatus}
        onProgress={onProgress}
      />
    </I18nProvider>,
  );
  return { onStatus, onProgress };
}

describe("console sequence handling", () => {
  beforeEach(() => {
    FakeSocket.instances = [];
    apiMock.openConsole.mockImplementation(async () => new FakeSocket() as unknown as WebSocket);
    apiMock.logs.mockResolvedValue([]);
  });

  it("deduplicates backfill/live overlap and forwards current progress", async () => {
    const { onProgress } = renderConsole();
    await waitFor(() => expect(FakeSocket.instances).toHaveLength(1));
    const socket = FakeSocket.instances[0];
    socket.open();

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
      first.open();
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

  it("requires a valid backfill before exposing connected state", async () => {
    renderConsole();

    await waitFor(() => expect(FakeSocket.instances).toHaveLength(1));
    const socket = FakeSocket.instances[0];
    socket.open();

    const connection = await screen.findByTestId("server-console-connection");
    expect(connection).toHaveAttribute("data-state", "reconnecting");
    socket.emit({
      type: "backfill",
      status: preparingStatus(),
      lines: [],
      through_seq: null,
    });
    await waitFor(() => expect(connection).toHaveAttribute("data-state", "connected"));
  });

  it("closes a socket that never delivers its initial backfill", async () => {
    vi.useFakeTimers();
    try {
      renderConsole();
      await vi.waitFor(() => expect(FakeSocket.instances).toHaveLength(1));
      const socket = FakeSocket.instances[0];
      socket.open();

      await vi.advanceTimersByTimeAsync(INITIAL_BACKFILL_TIMEOUT_MS);

      expect(socket.readyState).toBe(3);
      expect(screen.getByTestId("server-console-connection")).toHaveAttribute(
        "data-state",
        "reconnecting",
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows retained REST logs while disconnected and keeps commands disabled", async () => {
    apiMock.logs.mockResolvedValue([line(20), line(21)]);
    renderConsole("starting");

    expect(await screen.findByText("line-21")).toBeInTheDocument();
    expect(screen.getByText("line-20")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("Not connected")).toBeDisabled();
    expect(screen.getByTestId("server-console-connection")).toHaveAttribute(
      "data-state",
      "reconnecting",
    );
  });

  it("stops REST polling after a socket backfill and deduplicates lines", async () => {
    vi.useFakeTimers();
    try {
      apiMock.logs.mockResolvedValue([line(20), line(21)]);
      renderConsole("starting");
      await vi.waitFor(() => expect(apiMock.logs).toHaveBeenCalled());
      await vi.waitFor(() => expect(screen.getByText("line-21")).toBeInTheDocument());

      const socket = FakeSocket.instances[0];
      socket.open();
      socket.emit({
        type: "backfill",
        status: preparingStatus(),
        lines: [line(20), line(21), line(22)],
        through_seq: 22,
      });
      await vi.waitFor(() =>
        expect(screen.getByTestId("server-console-connection")).toHaveAttribute(
          "data-state",
          "connected",
        ),
      );
      expect(screen.getAllByText("line-20")).toHaveLength(1);
      expect(screen.getByText("line-22")).toBeInTheDocument();

      apiMock.logs.mockClear();
      await vi.advanceTimersByTimeAsync(10_000);
      expect(apiMock.logs).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("resets reconnect delay only after a valid backfill", async () => {
    vi.useFakeTimers();
    try {
      renderConsole("starting");
      await vi.waitFor(() => expect(FakeSocket.instances).toHaveLength(1));
      const first = FakeSocket.instances[0];
      first.open();
      first.emit({ type: "backfill", status: preparingStatus(), lines: [], through_seq: null });
      first.close();

      await vi.advanceTimersByTimeAsync(1_000);
      await vi.waitFor(() => expect(FakeSocket.instances).toHaveLength(2));
      const second = FakeSocket.instances[1];
      second.open();
      second.emit({ type: "backfill", status: preparingStatus(), lines: [], through_seq: null });
      second.close();

      await vi.advanceTimersByTimeAsync(1_000);
      expect(FakeSocket.instances).toHaveLength(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it("uses capped exponential reconnect backoff and terminal HTTP errors", () => {
    expect(consoleRetryDelay(0)).toBe(1_000);
    expect(consoleRetryDelay(1)).toBe(2_000);
    expect(consoleRetryDelay(5)).toBe(30_000);
    expect(consoleRetryDelay(20)).toBe(30_000);

    const denied = new ApiError("denied", 403);
    const unavailable = new ApiError("unavailable", 503);
    expect(isTerminalConsoleError(denied)).toBe(true);
    expect(isTerminalConsoleError(unavailable)).toBe(false);
    expect(consoleErrorKind(denied)).toBe("authorization");
    expect(consoleErrorKind(unavailable)).toBe("server");
  });
});
