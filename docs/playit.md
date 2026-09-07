# Playit Setup

No separate Playit installation is required. The panel ships an embedded Playit runtime.

## Embedded mode (default)

1. Start `mcpanel` and open the admin-only **Playit** section in the web UI (`#/playit`).
2. Sign in with your playit.gg email and password (plus authenticator code when the account uses TOTP). The panel keeps the session in `<data-dir>/playit/account-session.json` (owner-only on Unix); the password is never stored.
3. Click **Connect account**. The panel claims this machine's agent under your account with no browser redirect and waits for it to connect. The legacy **Connect** browser-claim link remains as a fallback.

Once connected, choose a server and create its tunnel. The panel uses TCP to `127.0.0.1:<server-port>`, stores the Playit tunnel id in `panel.json`, and polls the service so provisioning, disabled, drifted, and connected states are visible. The server settings page also exposes the same attach/detach controls. The **Create a tunnel** form on the **Tunnels** tab creates standalone tunnels (custom port, protocol, and loopback address) without attaching them to a server. Deleting a server or tunnel removes a panel-managed tunnel first when the service is available.

The Playit page is tabbed: **Overview** (one unified card: status, agent identity, runtime vs login account states, ownership, and lifecycle-appropriate actions), **Servers** (per-server tunnels), **Tunnels** (the tunnel catalog with its source authority), and **Account** (login, TOTP, owned agents).

The Overview labels the two account concepts separately: **Agent account** is the runtime/agent-side state, **Web account** is the direct playit.gg login. `Agent account: Verified` with `Ownership: Not verified` simply means the agent runs fine but no web login has verified which account owns it.

## Tunnel authorities

Tunnel operations use exactly one authority each — they never fall back to the other:

- **Agent** (agent secret): server attach, server detach, reconciliation, and managed Minecraft tunnels. The agent works without any account login.
- **Account** (playit.gg session): the tunnel catalog while logged in, and global tunnel deletion on the **Tunnels** tab.

`GET /playit/tunnels` returns a catalog (`available`, `source`, `tunnels`). Only the normal pre-running lifecycles (`NeedsClaim`, `Starting`, `Stopping`) report `available: false` with HTTP 200. A stopped or unreachable runtime, a broken daemon, an error state, or a playit.gg API outage while the agent should be running propagates as a real error instead of hiding behind a fake normal state.

Global deletion (`DELETE /playit/tunnels/{id}`) uses the account session only and refuses server-bound tunnels with `409`: disconnect those from the Servers tab, where agent authority applies. The server cleanup queue never converts an account deletion into an agent deletion.

## Repair and reconcile

When Playit and `panel.json` disagree, the panel heals the association instead of failing on it:

- **Repair** (re-attaching a server tunnel) adopts the stored tunnel in place when Playit still reports it — reassigning it to the current agent and destination when it drifted, even across an agent change — and recreates it through the stable `mcpanel:<server-id>` name when Playit no longer reports it.
- **Reconcile** (`POST /servers/{id}/playit/reconcile`) does the same healing without needing a display name: a remotely deleted tunnel or agent is adopted or recreated and the binding updated. It never deletes a tunnel.
- Disabled, incompatible (non-Java type/protocol), and ambiguous records cannot be healed automatically; they stay visible for an explicit repair retry or **Forget association**, which drops only the local binding.

## Account session vs agent secret

These are independent on purpose:

- **Signing out** (`DELETE /playit/auth/session`) removes the account login only. The agent keeps running and tunnels stay up.
- **Disconnecting the agent** (`POST /playit/agent/disconnect`) removes the local agent secret and stops the agent. The account login stays. It works while signed out.
- **Deleting an agent** (`DELETE /playit/agents/{id}`) destroys the remote playit.gg agent and needs an explicit tunnel strategy: move its tunnels to another agent (`move_to_agent`) or unassign them, optionally disabled. Deleting the agent the panel runs on is refused — disconnect it first.
- **Forgetting a server** (`POST /servers/{id}/playit/forget`) removes the local association only; the remote tunnel is left untouched.
- **Changing account** (`POST /playit/auth/change`) disconnects the agent, revokes the old session, logs into the new account, and claims a fresh agent for it, then reconciles bound servers best-effort. Old agent ids, tunnel ids, and sessions are never reused. When the old account owns the current agent, the switch abandons its tunnels and requires `acknowledge_managed_agent`. When the new account needs TOTP, the flow pauses with no credentials retained; completing `POST /playit/auth/totp` automatically claims the new agent and reconciles its servers. The paused change is bound to the new account id and is dropped on logout, on a fresh change, when the TOTP attempt expires, or when a later unrelated login is active, so it can never run under the wrong account.

`GET /playit/agent/ownership` reports whether the runtime agent belongs to the logged-in account (`matched`, `different_account`, `no_agent`, `unknown`). Under `different_account`, generic tunnel creation, Minecraft tunnel creation, server attach, repair, reconciliation, and tunnel reassignment are blocked with `409` so one account can never corrupt another account's tunnels. Agent-only operation without any login stays allowed.

## External mode (legacy IPC)

Operators who intentionally run a compatible external `playitd` can select the legacy IPC backend explicitly. Keep the listener on loopback, or explicitly acknowledge the risk with `--allow-insecure-http` on an isolated network:

```sh
mcpanel --playit-mode external --data-dir ./data --bind 127.0.0.1:8080
```

The `MCPANEL_PLAYIT_MODE=external` environment variable is equivalent. External mode does not stop the independently managed daemon when the panel exits.

## Credentials and switching modes

Embedded and external modes keep separate Playit credentials. Switching from an external daemon does not import its secret automatically, so embedded mode may require a new claim. Existing panel tunnel bindings are preserved and reconciled against whichever Playit account is active.

## Runtime boundary

The panel HTTP layer only translates requests/responses; all Playit semantics live in `crates/playit-integration`.

```
panel (mcpanel)
    │
    └── PlayitManager
          ├── agent side: embedded PlayitRuntime (default)
          │               or external playitd IPC (optional)
          └── account side: direct playit.gg API (Bearer session)
                ├── sign_in / TOTP / validate / logout
                ├── browserless claim approve/reject + direct setup
                ├── agents list/delete, domains, account tunnels
                └── persisted via FileSessionStore, never the password
```

## API

See [API — Playit endpoints](api.md#playit) and [Security](security.md) for the trust implications of exposing a Minecraft port.

## Troubleshooting

- If a tunnel shows `drifted`, the remote Playit state no longer matches `panel.json`; use **Repair tunnel** (or **Reconcile**) on the Playit page to adopt it back in place.
- If a tunnel shows `missing` (deleted on the Playit website) or the account/agent changed remotely, **Reconcile** recreates the tunnel and updates the binding. If the agent itself was deleted, claim the agent again first (direct **Connect account** or the browser claim), then reconcile.
- Switching modes requires a claim in the new mode; bindings survive the switch.
