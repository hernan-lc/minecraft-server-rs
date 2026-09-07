import { fireEvent, render, screen, waitFor } from "@testing-library/preact";
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "../i18n";
import { DialogProvider } from "../components/Modal";
import { ToastProvider } from "../components/Toast";
import { Playit } from "./Playit";
import type { Server } from "../types";

const apiMock = vi.hoisted(() => ({
  playitStatus: vi.fn(),
  playitAccount: vi.fn(),
  playitTunnels: vi.fn(),
  servers: vi.fn(),
  playitClaim: vi.fn(),
  attachPlayit: vi.fn(),
  detachPlayit: vi.fn(),
  deletePlayitTunnel: vi.fn(),
}));

vi.mock("../api", () => ({ api: apiMock }));

const server = {
  id: "server-1",
  name: "Survival",
  port: 25565,
  playit: null,
} as Server;

const connectedStatus = (version: string) => ({
  status: "connected" as const,
  version,
  message: null,
});

const verifiedAccount = {
  status: "verified" as const,
  agent_id: "agent-1",
  login_link: null,
  claim_url: null,
};

function renderPlayit() {
  return render(
    <I18nProvider>
      <ToastProvider>
        <DialogProvider>
          <Playit />
        </DialogProvider>
      </ToastProvider>
    </I18nProvider>,
  );
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

beforeEach(() => {
  vi.clearAllMocks();
  apiMock.playitStatus.mockResolvedValue(connectedStatus("initial"));
  apiMock.playitAccount.mockResolvedValue(verifiedAccount);
  apiMock.playitTunnels.mockResolvedValue([]);
  apiMock.servers.mockResolvedValue([server]);
  apiMock.playitClaim.mockResolvedValue({ claim_url: "https://playit.gg/claim/test" });
  apiMock.attachPlayit.mockResolvedValue({});
  apiMock.detachPlayit.mockResolvedValue({
    state: "disabled",
    binding: null,
    tunnel: null,
    message: null,
    cleanup_pending: false,
  });
  apiMock.deletePlayitTunnel.mockResolvedValue({ ok: true });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("Playit page refresh lifecycle", () => {
  it("does not let an older refresh overwrite a newer result", async () => {
    apiMock.playitStatus.mockResolvedValueOnce({
      status: "needs_claim",
      version: "initial",
      message: null,
    });
    apiMock.playitAccount.mockResolvedValueOnce({
      status: "guest",
      agent_id: null,
      login_link: null,
      claim_url: null,
    });

    renderPlayit();
    await waitFor(() => expect(screen.getByText("initial")).toBeInTheDocument());

    const claimRequest = deferred<{ claim_url: string }>();
    apiMock.playitClaim.mockImplementationOnce(() => claimRequest.promise);

    const oldStatus = deferred<ReturnType<typeof connectedStatus>>();
    const oldAccount = deferred<typeof verifiedAccount>();
    const oldTunnels = deferred<never[]>();
    const oldServers = deferred<Server[]>();
    const newStatus = connectedStatus("newer");

    apiMock.playitStatus
      .mockImplementationOnce(() => oldStatus.promise)
      .mockResolvedValueOnce(newStatus);
    apiMock.playitAccount
      .mockImplementationOnce(() => oldAccount.promise)
      .mockResolvedValueOnce(verifiedAccount);
    apiMock.playitTunnels
      .mockImplementationOnce(() => oldTunnels.promise)
      .mockResolvedValueOnce([]);
    apiMock.servers
      .mockImplementationOnce(() => oldServers.promise)
      .mockResolvedValueOnce([server]);

    fireEvent.click(screen.getByRole("button", { name: "Connect Playit" }));
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    // Completing the claim performs another refresh while the manual refresh
    // is still pending, exercising the same overlap as polling plus actions.
    claimRequest.resolve({ claim_url: "https://playit.gg/claim/test" });

    oldStatus.resolve(connectedStatus("old"));
    oldAccount.resolve(verifiedAccount);
    oldTunnels.resolve([]);
    oldServers.resolve([server]);

    await waitFor(() => expect(apiMock.playitStatus).toHaveBeenCalledTimes(3));
    await waitFor(() => expect(screen.getByText("newer")).toBeInTheDocument());
    expect(screen.queryByText("old")).toBeNull();
  });

  it("labels the action as connecting rather than always creating", async () => {
    renderPlayit();
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Connect server" })).toBeInTheDocument(),
    );
  });

  it.each([
    ["reused", "Existing Playit tunnel reused."],
    ["updated", "Existing Playit tunnel updated."],
    ["created", "New Playit tunnel created."],
    [undefined, "New Playit tunnel created."],
  ] as const)("announces attach disposition %s as %s", async (disposition, message) => {
    apiMock.attachPlayit.mockResolvedValue({
      state: "connected",
      binding: null,
      tunnel: null,
      message: null,
      cleanup_pending: false,
      disposition,
    });

    renderPlayit();
    await waitFor(() =>
      expect(
        screen.getByRole("option", { name: "Survival · :25565" }),
      ).toBeInTheDocument(),
    );

    // Drive the controlled select the way a user interaction does: pick the
    // option, then dispatch the bubbled change Preact listens for.
    const combo = screen.getByRole("combobox") as HTMLSelectElement;
    combo.value = "server-1";
    combo.dispatchEvent(new Event("change", { bubbles: true }));
    // Preact flushes state asynchronously; wait for the selection to commit
    // before clicking, since a disabled button ignores clicks.
    const button = screen.getByRole("button", { name: "Connect server" });
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.click(button);

    await waitFor(() => expect(apiMock.attachPlayit).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByText(message)).toBeInTheDocument());
  });

  it("explains tunnel load failures as a claim step while the agent needs setup", async () => {
    apiMock.playitStatus.mockResolvedValue({
      status: "needs_claim",
      version: "1.0.10",
      message: null,
    });
    apiMock.playitAccount.mockResolvedValue({
      status: "guest",
      agent_id: null,
      login_link: null,
      claim_url: null,
    });
    apiMock.playitTunnels.mockRejectedValue(
      new Error("Playit is temporarily unavailable. Try again shortly."),
    );

    renderPlayit();

    await waitFor(() =>
      expect(
        screen.getByText("Claim your Playit agent above to load tunnels."),
      ).toBeInTheDocument(),
    );
    expect(
      screen.queryByText("Playit is temporarily unavailable. Try again shortly."),
    ).toBeNull();
  });

  it("clears a local claim URL once the account is connected", async () => {
    apiMock.playitStatus
      .mockResolvedValueOnce({ status: "needs_claim", version: "1", message: null })
      .mockResolvedValueOnce(connectedStatus("1"));
    apiMock.playitAccount
      .mockResolvedValueOnce({
        status: "guest",
        agent_id: null,
        login_link: null,
        claim_url: null,
      })
      .mockResolvedValueOnce(verifiedAccount);

    renderPlayit();
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Connect Playit" })).toBeInTheDocument(),
    );

    fireEvent.click(screen.getByRole("button", { name: "Connect Playit" }));
    await waitFor(() => expect(apiMock.playitClaim).toHaveBeenCalledOnce());
    await waitFor(() => expect(screen.queryByRole("link", { name: "Open Playit claim" })).toBeNull());
  });
});
