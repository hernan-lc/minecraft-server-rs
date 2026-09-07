import { fireEvent, render, screen, waitFor, within } from "@testing-library/preact";
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "../i18n";
import { DialogProvider } from "../components/Modal";
import { ToastProvider } from "../components/Toast";
import { Playit } from "./Playit";
import type { Server } from "../types";

const loggedOutSession = {
  authenticated: false,
  requires_totp: false,
  account_id: null,
  account_status: null,
  read_only: false,
};

const apiMock = vi.hoisted(() => ({
  playitStatus: vi.fn(),
  playitAccount: vi.fn(),
  playitTunnels: vi.fn(),
  createPlayitTunnel: vi.fn(),
  servers: vi.fn(),
  serverPlayit: vi.fn(),
  playitClaim: vi.fn(),
  attachPlayit: vi.fn(),
  detachPlayit: vi.fn(),
  reconcilePlayit: vi.fn(),
  forgetPlayit: vi.fn(),
  deletePlayitTunnel: vi.fn(),
  playitAuthSession: vi.fn(),
  playitAuthLogin: vi.fn(),
  playitAuthTotp: vi.fn(),
  playitAuthLogout: vi.fn(),
  playitAuthChange: vi.fn(),
  playitSetupDirect: vi.fn(),
  playitAgents: vi.fn(),
  playitDeleteAgent: vi.fn(),
  playitAgentDisconnect: vi.fn(),
  playitAgentReconnect: vi.fn(),
  playitOwnership: vi.fn(),
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

const emptyCatalog = { available: true, source: "account", tunnels: [] };

function openTab(name: "Overview" | "Servers" | "Tunnels" | "Account") {
  fireEvent.click(screen.getByRole("tab", { name }));
}

beforeEach(() => {
  vi.clearAllMocks();
  apiMock.playitStatus.mockResolvedValue(connectedStatus("initial"));
  apiMock.playitAccount.mockResolvedValue(verifiedAccount);
  apiMock.playitTunnels.mockResolvedValue({ ...emptyCatalog });
  apiMock.playitOwnership.mockResolvedValue({ ownership: "unknown", agent_id: null });
  apiMock.servers.mockResolvedValue([server]);
  apiMock.playitClaim.mockResolvedValue({ claim_url: "https://playit.gg/claim/test" });
  apiMock.serverPlayit.mockResolvedValue({
    state: "disabled",
    binding: null,
    tunnel: null,
    message: null,
    cleanup_pending: false,
  });
  apiMock.attachPlayit.mockResolvedValue({});
  apiMock.reconcilePlayit.mockResolvedValue({
    state: "connected",
    binding: null,
    tunnel: null,
    message: null,
    cleanup_pending: false,
  });
  apiMock.forgetPlayit.mockResolvedValue({
    state: "disabled",
    binding: null,
    tunnel: null,
    message: null,
    cleanup_pending: false,
  });
  apiMock.detachPlayit.mockResolvedValue({
    state: "disabled",
    binding: null,
    tunnel: null,
    message: null,
    cleanup_pending: false,
  });
  apiMock.deletePlayitTunnel.mockResolvedValue({ ok: true, cleanup_pending: false });
  apiMock.playitAuthSession.mockResolvedValue({ ...loggedOutSession });
  apiMock.playitAgents.mockResolvedValue([]);
  apiMock.playitAuthChange.mockResolvedValue({
    session: {
      authenticated: true,
      requires_totp: false,
      account_id: 9,
      account_status: "verified",
      read_only: false,
    },
    setup: { agent_id: "agent-9", already_configured: false, connected: true, message: null },
    ownership_before: "unknown",
    servers_recovered: 0,
    servers_total: 0,
  });
  apiMock.createPlayitTunnel.mockResolvedValue({ tunnel_id: "tunnel-9", message: null });
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
    // The first refresh must fully settle (loading clears last) before the
    // overlap starts. Otherwise its in-flight requests consume the mocks
    // queued below for the overlapping refreshes.
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Refresh" })).not.toBeDisabled(),
    );

    const claimRequest = deferred<{ claim_url: string }>();
    apiMock.playitClaim.mockImplementationOnce(() => claimRequest.promise);

    const oldStatus = deferred<ReturnType<typeof connectedStatus>>();
    const oldAccount = deferred<typeof verifiedAccount>();
    const oldTunnels = deferred<{ available: boolean; source: string; tunnels: never[] }>();
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
      .mockResolvedValueOnce({ ...emptyCatalog });
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
    oldTunnels.resolve({ ...emptyCatalog });
    oldServers.resolve([server]);

    await waitFor(() => expect(apiMock.playitStatus).toHaveBeenCalledTimes(3));
    await waitFor(() => expect(screen.getByText("newer")).toBeInTheDocument());
    expect(screen.queryByText("old")).toBeNull();
  });

  it("renders every server as its own row with a connect action", async () => {
    renderPlayit();
    openTab("Servers");
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Connect server: Survival" }),
      ).toBeInTheDocument(),
    );
    expect(apiMock.serverPlayit).toHaveBeenCalledWith("server-1");
  });

  it("shows an empty state instead of rows when no servers exist", async () => {
    apiMock.servers.mockResolvedValue([]);
    renderPlayit();
    openTab("Servers");
    await waitFor(() =>
      expect(
        screen.getByText("No servers yet. Create one from the dashboard, then connect it here."),
      ).toBeInTheDocument(),
    );
    expect(apiMock.serverPlayit).not.toHaveBeenCalled();
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
    openTab("Servers");
    const button = await screen.findByRole("button", { name: "Connect server: Survival" });
    fireEvent.click(button);

    await waitFor(() => expect(apiMock.attachPlayit).toHaveBeenCalledWith("server-1"));
    await waitFor(() => expect(screen.getByText(message)).toBeInTheDocument());
  });

  it("offers repair and reconcile actions for a drifted server tunnel", async () => {
    apiMock.serverPlayit.mockResolvedValue({
      state: "drifted",
      binding: {
        tunnel_id: "tunnel-1",
        protocol: "tcp",
        local_address: "127.0.0.1",
        local_port: 25565,
        agent_id: "agent-1",
        created_at: null,
      },
      tunnel: {
        id: "tunnel-1",
        name: "mcpanel:server-1",
        display_address: "example.playit.gg:1234",
        destination: "127.0.0.1:25566",
        protocol: "tcp",
        tunnel_type: "minecraft-java",
        agent_id: "agent-1",
        local_address: "127.0.0.1",
        local_port: 25566,
        disabled: false,
        disabled_reason: null,
      },
      message: "The Playit destination is missing or differs from the server port",
      cleanup_pending: false,
    });
    apiMock.attachPlayit.mockResolvedValue({
      state: "connected",
      binding: null,
      tunnel: null,
      message: null,
      cleanup_pending: false,
      disposition: "updated",
    });

    renderPlayit();
    openTab("Servers");
    const repair = await screen.findByRole("button", { name: "Repair tunnel: Survival" });
    expect(
      screen.getByRole("button", { name: "Reconcile: Survival" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Connect server: Survival" }),
    ).toBeNull();

    fireEvent.click(repair);
    await waitFor(() => expect(apiMock.attachPlayit).toHaveBeenCalledWith("server-1"));
    await waitFor(() =>
      expect(screen.getByText("Existing Playit tunnel updated.")).toBeInTheDocument(),
    );
  });

  it("reconciles a broken tunnel from its row", async () => {
    apiMock.serverPlayit.mockResolvedValue({
      state: "missing",
      binding: {
        tunnel_id: "tunnel-1",
        protocol: "tcp",
        local_address: "127.0.0.1",
        local_port: 25565,
        agent_id: "agent-1",
        created_at: null,
      },
      tunnel: null,
      message: "The stored Playit tunnel is missing; repair or forget the association",
      cleanup_pending: false,
    });

    renderPlayit();
    openTab("Servers");
    fireEvent.click(
      await screen.findByRole("button", { name: "Reconcile: Survival" }),
    );

    await waitFor(() => expect(apiMock.reconcilePlayit).toHaveBeenCalledWith("server-1"));
    await waitFor(() =>
      expect(screen.getByText("Playit association reconciled.")).toBeInTheDocument(),
    );
  });

  it("shows the public address with a copy action for connected servers", async () => {
    apiMock.serverPlayit.mockResolvedValue({
      state: "connected",
      binding: {
        tunnel_id: "tunnel-1",
        protocol: "tcp",
        local_address: "127.0.0.1",
        local_port: 25565,
        agent_id: "agent-1",
        created_at: null,
      },
      tunnel: {
        id: "tunnel-1",
        name: "mcpanel:server-1",
        display_address: "example.playit.gg:1234",
        destination: "127.0.0.1:25565",
        protocol: "tcp",
        tunnel_type: "minecraft-java",
        agent_id: "agent-1",
        local_address: "127.0.0.1",
        local_port: 25565,
        disabled: false,
        disabled_reason: null,
      },
      message: null,
      cleanup_pending: false,
    });
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });

    renderPlayit();
    openTab("Servers");
    await waitFor(() =>
      expect(screen.getByText("example.playit.gg:1234")).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Copy address" }));

    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith("example.playit.gg:1234"),
    );
    await waitFor(() =>
      expect(screen.getByText("Address copied.")).toBeInTheDocument(),
    );
  });

  it("shows an unavailable catalog as a setup step instead of an error", async () => {
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
    apiMock.playitTunnels.mockResolvedValue({ available: false, source: "none", tunnels: [] });

    renderPlayit();
    openTab("Tunnels");

    await waitFor(() =>
      expect(
        screen.getByText("Tunnel listing is unavailable while Playit is starting."),
      ).toBeInTheDocument(),
    );
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

describe("Playit account card", () => {
  it("opens the sign-in modal when logged out", async () => {
    renderPlayit();
    openTab("Account");
    await waitFor(() => expect(apiMock.playitAuthSession).toHaveBeenCalled());
    // Minimalist card: no inline inputs, a single action opens the modal.
    expect(screen.queryByLabelText("Email")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByLabelText("Email")).toBeInTheDocument();
    expect(screen.getByLabelText("Password")).toBeInTheDocument();
  });

  it("signs in and loads the agents", async () => {
    const session = {
      authenticated: true,
      requires_totp: false,
      account_id: 7,
      account_status: "verified",
      read_only: false,
    };
    let current: {
      authenticated: boolean;
      requires_totp: boolean;
      account_id: number | null;
      account_status: string | null;
      read_only: boolean;
    } = { ...loggedOutSession };
    apiMock.playitAuthSession.mockImplementation(() => Promise.resolve({ ...current }));
    apiMock.playitAuthLogin.mockImplementation(() => {
      current = { ...session };
      return Promise.resolve({ ...session });
    });
    apiMock.playitAgents.mockResolvedValue([{ id: "agent-1", name: "one" }]);

    renderPlayit();
    openTab("Account");
    await waitFor(() => expect(apiMock.playitAuthSession).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    await waitFor(() => expect(screen.getByLabelText("Email")).toBeInTheDocument());
    fireEvent.input(screen.getByLabelText("Email"), { target: { value: "user@example.com" } });
    fireEvent.input(screen.getByLabelText("Password"), { target: { value: "secret" } });
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Sign in" }));

    await waitFor(() =>
      expect(apiMock.playitAuthLogin).toHaveBeenCalledWith("user@example.com", "secret"),
    );
    await waitFor(() => expect(screen.getByText("Account agents")).toBeInTheDocument());
    expect(screen.getByText("one")).toBeInTheDocument();
  });

  it("asks for a TOTP code when the login requires it", async () => {
    const verified = {
      authenticated: true,
      requires_totp: false,
      account_id: 7,
      account_status: "verified",
      read_only: false,
    };
    const pending = {
      authenticated: false,
      requires_totp: true,
      account_id: 7,
      account_status: "verified",
      read_only: false,
    };
    let current: {
      authenticated: boolean;
      requires_totp: boolean;
      account_id: number | null;
      account_status: string | null;
      read_only: boolean;
    } = { ...loggedOutSession };
    apiMock.playitAuthSession.mockImplementation(() => Promise.resolve({ ...current }));
    apiMock.playitAuthLogin.mockImplementation(() => {
      current = { ...pending };
      return Promise.resolve({ ...pending });
    });
    apiMock.playitAuthTotp.mockImplementation(() => {
      current = { ...verified };
      return Promise.resolve({
        session: { ...verified },
        setup: null,
        servers_recovered: 0,
        servers_total: 0,
        setup_error: null,
      });
    });

    renderPlayit();
    openTab("Account");
    await waitFor(() => expect(apiMock.playitAuthSession).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    await waitFor(() => expect(screen.getByLabelText("Email")).toBeInTheDocument());
    fireEvent.input(screen.getByLabelText("Email"), { target: { value: "user@example.com" } });
    fireEvent.input(screen.getByLabelText("Password"), { target: { value: "secret" } });
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Sign in" }));

    const code = await screen.findByLabelText("Authenticator code", { exact: false });
    fireEvent.input(code, { target: { value: "123456" } });
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Verify" }));
    await waitFor(() => expect(apiMock.playitAuthTotp).toHaveBeenCalledWith("123456"));
  });

  it("shows an empty agents card when signed in without agents", async () => {
    apiMock.playitAuthSession.mockResolvedValue({
      authenticated: true,
      requires_totp: false,
      account_id: 7,
      account_status: "verified",
      read_only: false,
    });
    apiMock.playitAgents.mockResolvedValue([]);

    renderPlayit();
    openTab("Account");
    await waitFor(() =>
      expect(
        screen.getByText("No agents are registered on this account yet."),
      ).toBeInTheDocument(),
    );
  });

  it("offers direct setup while the agent needs claiming", async () => {
    apiMock.playitStatus.mockResolvedValue({
      status: "needs_claim",
      version: null,
      message: null,
    });
    apiMock.playitAuthSession.mockResolvedValue({
      authenticated: true,
      requires_totp: false,
      account_id: 7,
      account_status: "verified",
      read_only: false,
    });
    apiMock.playitSetupDirect.mockResolvedValue({
      agent_id: "agent-9",
      already_configured: false,
      connected: true,
      message: null,
    });

    renderPlayit();
    const button = await screen.findByRole("button", { name: "Connect this device" });
    fireEvent.click(button);
    await waitFor(() => expect(apiMock.playitSetupDirect).toHaveBeenCalledOnce());
  });
});

describe("Playit tunnel creation", () => {
  it("creates a standalone tunnel from the modal form", async () => {
    renderPlayit();
    openTab("Tunnels");
    fireEvent.click(await screen.findByRole("button", { name: "Create tunnel" }));
    fireEvent.input(await screen.findByLabelText("Name (optional)"), {
      target: { value: "lobby" },
    });
    fireEvent.input(await screen.findByPlaceholderText("25565"), {
      target: { value: "25566" },
    });
    // Selects listen for `input` (which browsers fire on selection change);
    // `change` events do not reach Preact handlers in this jsdom setup.
    fireEvent.input(screen.getByLabelText("Protocol"), { target: { value: "udp" } });
    fireEvent.input(screen.getByLabelText("Local address"), {
      target: { value: "::1" },
    });
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "Create tunnel" }),
    );

    await waitFor(() =>
      expect(apiMock.createPlayitTunnel).toHaveBeenCalledWith({
        local_port: 25566,
        protocol: "udp",
        local_address: "::1",
        name: "lobby",
      }),
    );
    await waitFor(() =>
      expect(screen.getByText("New Playit tunnel created.")).toBeInTheDocument(),
    );
  });

  it("rejects an invalid port without calling the API", async () => {
    renderPlayit();
    openTab("Tunnels");
    fireEvent.click(await screen.findByRole("button", { name: "Create tunnel" }));
    fireEvent.input(await screen.findByPlaceholderText("25565"), {
      target: { value: "70000" },
    });
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "Create tunnel" }),
    );

    await waitFor(() =>
      expect(screen.getByText("Enter a port between 1 and 65535.")).toBeInTheDocument(),
    );
    expect(apiMock.createPlayitTunnel).not.toHaveBeenCalled();
  });

  it("disables creation while Playit is not connected", async () => {
    apiMock.playitStatus.mockResolvedValue({
      status: "needs_claim",
      version: null,
      message: null,
    });

    renderPlayit();
    openTab("Tunnels");
    const button = await screen.findByRole("button", { name: "Create tunnel" });
    expect(button).toBeDisabled();
    await waitFor(() =>
      expect(
        screen.getByText("Connect Playit before creating a tunnel."),
      ).toBeInTheDocument(),
    );
  });
});

describe("Playit tabs and ownership", () => {
  it("renders four tabs with overview selected by default", async () => {
    renderPlayit();
    for (const name of ["Overview", "Servers", "Tunnels", "Account"]) {
      expect(screen.getByRole("tab", { name })).toBeInTheDocument();
    }
    expect(screen.getByRole("tab", { name: "Overview" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByText("Playit overview")).toBeInTheDocument();
  });

  it("keeps agent controls available while logged out", async () => {
    renderPlayit();
    await waitFor(() => expect(apiMock.playitAuthSession).toHaveBeenCalled());
    expect(
      screen.getByRole("button", { name: "Disconnect agent" }),
    ).toBeInTheDocument();
    // Reconnect is hidden while already connected; Change stays available.
    expect(
      screen.queryByRole("button", { name: "Reconnect agent" }),
    ).toBeNull();
    expect(
      await screen.findByRole("button", { name: "Change account" }),
    ).toBeInTheDocument();
  });

  it("warns when the connected agent belongs to another account", async () => {
    apiMock.playitOwnership.mockResolvedValue({
      ownership: "different_account",
      agent_id: "agent-9",
    });
    renderPlayit();
    await waitFor(() =>
      expect(
        screen.getByText(
          "This agent belongs to another Playit account.",
        ),
      ).toBeInTheDocument(),
    );
  });

  it("shows the tunnel source on the tunnels tab", async () => {
    apiMock.playitTunnels.mockResolvedValue({ available: true, source: "agent", tunnels: [] });
    renderPlayit();
    openTab("Tunnels");
    await waitFor(() =>
      expect(screen.getByText("Local agent", { exact: false })).toBeInTheDocument(),
    );
  });

  it("disables global deletes while logged out", async () => {
    apiMock.playitTunnels.mockResolvedValue({
      available: true,
      source: "agent",
      tunnels: [
        {
          id: "tunnel-1",
          name: null,
          display_address: "example.playit.gg:1",
          destination: "127.0.0.1:25565",
          protocol: "tcp",
          tunnel_type: "minecraft-java",
          agent_id: "agent-1",
          local_address: "127.0.0.1",
          local_port: 25565,
          disabled: false,
          disabled_reason: null,
        },
      ],
    });
    renderPlayit();
    openTab("Tunnels");
    const button = await screen.findByRole("button", { name: "Delete" });
    expect(button).toBeDisabled();
  });

  it("changes account through the modal", async () => {
    renderPlayit();
    await waitFor(() => expect(apiMock.playitAuthSession).toHaveBeenCalled());
    fireEvent.click(await screen.findByRole("button", { name: "Change account" }));
    fireEvent.input(await screen.findByLabelText("Email"), {
      target: { value: "new@example.com" },
    });
    fireEvent.input(screen.getByLabelText("Password"), {
      target: { value: "secret" },
    });
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "Change account" }),
    );

    await waitFor(() =>
      expect(apiMock.playitAuthChange).toHaveBeenCalledWith("new@example.com", "secret", {}),
    );
    await waitFor(() =>
      expect(screen.getByText("Playit account changed.")).toBeInTheDocument(),
    );
  });

  it("asks for acknowledgement when the old account owns the agent", async () => {
    const conflict = Object.assign(
      new Error("the current account owns this agent; acknowledge the switch"),
      { status: 409 },
    );
    apiMock.playitAuthChange.mockRejectedValueOnce(conflict);
    renderPlayit();
    await waitFor(() => expect(apiMock.playitAuthSession).toHaveBeenCalled());
    fireEvent.click(await screen.findByRole("button", { name: "Change account" }));
    fireEvent.input(await screen.findByLabelText("Email"), {
      target: { value: "new@example.com" },
    });
    fireEvent.input(screen.getByLabelText("Password"), {
      target: { value: "secret" },
    });
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Change account" }));

    await waitFor(() =>
      expect(
        screen.getByText("Switching abandons its tunnels. Switch anyway?", { exact: false }),
      ).toBeInTheDocument(),
    );
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Change account" }));
    await waitFor(() =>
      expect(apiMock.playitAuthChange).toHaveBeenLastCalledWith("new@example.com", "secret", {
        acknowledge_managed_agent: true,
      }),
    );
  });
});

describe("Playit overview states", () => {
  it("shows the agent ID exactly once on the overview", async () => {
    renderPlayit();
    expect(await screen.findAllByText("agent-1")).toHaveLength(1);
  });

  it("labels runtime and login account states separately while signed out", async () => {
    renderPlayit();
    await screen.findByText("Agent account");
    await screen.findByText("Verified");
    expect(screen.getByText("Signed out")).toBeInTheDocument();
    expect(screen.getByText("Not verified", { exact: false })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Disconnect agent" }),
    ).toBeInTheDocument();
  });

  it("confirms a matched agent ownership", async () => {
    apiMock.playitAuthSession.mockResolvedValue({
      authenticated: true,
      requires_totp: false,
      account_id: 7,
      account_status: "verified",
      read_only: false,
    });
    apiMock.playitOwnership.mockResolvedValue({ ownership: "matched", agent_id: "agent-1" });
    renderPlayit();
    await waitFor(() =>
      expect(
        screen.getByText("This agent belongs to the signed-in account."),
      ).toBeInTheDocument(),
    );
  });

  it("disables management actions for a foreign account", async () => {
    apiMock.playitAuthSession.mockResolvedValue({
      authenticated: true,
      requires_totp: false,
      account_id: 7,
      account_status: "verified",
      read_only: false,
    });
    apiMock.playitOwnership.mockResolvedValue({
      ownership: "different_account",
      agent_id: "agent-9",
    });
    renderPlayit();
    openTab("Servers");
    const connect = await screen.findByRole("button", { name: "Connect server: Survival" });
    expect(connect).toBeDisabled();
    expect(connect.title).toBe(
      "The connected agent belongs to another Playit account. Change account before managing server tunnels.",
    );
    openTab("Tunnels");
    expect(await screen.findByRole("button", { name: "Create tunnel" })).toBeDisabled();
    openTab("Overview");
    expect(
      await screen.findByText("This agent belongs to another Playit account."),
    ).toBeInTheDocument();
    expect(
      await screen.findByRole("button", { name: "Change account" }),
    ).toBeInTheDocument();
  });

  it("supports arrow-key tab navigation", async () => {
    renderPlayit();
    screen.getByRole("tab", { name: "Overview" }).focus();
    fireEvent.keyDown(screen.getByRole("tablist"), { key: "ArrowRight" });
    expect(screen.getByRole("tab", { name: "Servers" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Servers" }));
    expect(screen.getByRole("tabpanel")).toHaveAttribute("id", "playit-panel-servers");
  });

  it("continues an account change automatically after TOTP", async () => {
    apiMock.playitAuthChange.mockResolvedValue({
      session: {
        authenticated: false,
        requires_totp: true,
        account_id: null,
        account_status: null,
        read_only: false,
      },
      setup: null,
      ownership_before: "unknown",
      servers_recovered: 0,
      servers_total: 1,
    });
    apiMock.playitAuthTotp.mockResolvedValue({
      session: {
        authenticated: true,
        requires_totp: false,
        account_id: 9,
        account_status: "verified",
        read_only: false,
      },
      setup: { agent_id: "agent-9", already_configured: false, connected: true, message: null },
      servers_recovered: 1,
      servers_total: 1,
      setup_error: null,
    });
    renderPlayit();
    fireEvent.click(await screen.findByRole("button", { name: "Change account" }));
    fireEvent.input(await screen.findByLabelText("Email"), {
      target: { value: "new@example.com" },
    });
    fireEvent.input(screen.getByLabelText("Password"), {
      target: { value: "secret" },
    });
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "Change account" }),
    );

    // TOTP pending lands on the account tab with the code dialog open.
    const code = await screen.findByLabelText("Authenticator code", { exact: false });
    fireEvent.input(code, { target: { value: "123456" } });
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Verify" }));

    await waitFor(() =>
      expect(screen.getByText("Playit agent connected.")).toBeInTheDocument(),
    );
    await waitFor(() =>
      expect(screen.getByText("Recovered 1 of 1 server tunnels.")).toBeInTheDocument(),
    );
  });
});
