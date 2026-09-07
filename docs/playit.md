# Playit Setup

No separate Playit installation is required. The panel ships an embedded Playit runtime.

## Embedded mode (default)

1. Start `mcpanel` and open the admin-only **Playit** section in the web UI (`#/playit`).
2. Sign in with your playit.gg email and password (plus authenticator code when the account uses TOTP). The panel keeps the session in `<data-dir>/playit/account-session.json` (owner-only on Unix); the password is never stored.
3. Click **Connect account**. The panel claims this machine's agent under your account with no browser redirect and waits for it to connect. The legacy **Connect** browser-claim link remains as a fallback.

Once connected, choose a server and create its tunnel. The panel uses TCP to `127.0.0.1:<server-port>`, stores the Playit tunnel id in `panel.json`, and polls the service so provisioning, disabled, drifted, and connected states are visible. The server settings page also exposes the same attach/detach controls. Deleting a server or tunnel removes a panel-managed tunnel first when the service is available.

## Account session vs agent secret

These are independent on purpose:

- **Signing out** (`DELETE /playit/auth/session`) deletes the account login only. The agent keeps running and tunnels stay up.
- **Disconnecting the agent** (`POST /playit/agent/disconnect`) removes the agent secret and restarts the embedded runtime into setup mode. The account login stays.
- **Deleting an agent** (`DELETE /playit/agents/{id}`) is destructive and needs an explicit tunnel strategy: move its tunnels to another agent (`move_to_agent`) or unassign them, optionally disabled. Deleting the agent the panel runs on is refused — disconnect it first.

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

- If a tunnel shows `drifted`, the remote Playit state no longer matches `panel.json`; re-attach from the server settings or recreate the tunnel.
- Switching modes requires a claim in the new mode; bindings survive the switch.
