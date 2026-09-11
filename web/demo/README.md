# mcpanel demo automation (Playwright)

Automated, reproducible mcpanel demo workflows using Playwright.
The first workflow drives the real web UI from first run to server creation
and records it to video — with zero configuration.

## Purpose

- Reproducible automated demos of the real mcpanel UI.
- A foundation for future browser workflow/regression testing.
- Fully deterministic: stable `data-testid` locators, no AI, no API keys,
  no cloud sessions.

The password is filled directly with a locator and never logged.

## Requirements

- Node/npm.
- A running mcpanel serving the UI (default `http://127.0.0.1:8080`).
- Playwright's Chromium: `npx playwright install chromium` (one time).

## Recording a demo

No environment setup needed. Every run records
`artifacts/demos/first-run.webm` (1280×720):

```bash
# terminal 1 — serve the app from a FRESH data directory
./target/release/mcpanel --data-dir ./data-demo --bind 127.0.0.1:8080
# (or the appropriate existing development command)

# terminal 2
cd web
npm install
npm run demo:first-run
```

Output:

```text
[demo] target: http://127.0.0.1:8080
[demo] opening setup
[demo] creating admin
[demo] logging in
[demo] opening new server
[demo] creating Survival
[demo] minecraft: 26.2 (latest of 66)
[demo] java: 25
[demo] server visible
[demo] recording saved: .../artifacts/demos/first-run.webm
[demo] success
```

To watch it live while it records:

```bash
npm run demo:first-run:headed
```

which is shorthand for `tsx demo/first-run.ts --headed --slow`.
`--slow` adds 250/500/1000 ms pacing so the footage is watchable;
without it the run is as fast as possible.

## Options (all optional)

CLI flags (highest precedence):

```text
--headed / --headless   show the browser or not (default: headless)
--slow                  human-readable pacing for published demos
--url <url>             panel URL
--user <name>           demo username (default: admin)
--password <pw>         demo password (default: mcpanel-demo-password)
```

Environment variables (or a `web/.env.demo` file, see
`.env.demo.example`) use the same values under `MCPANEL_DEMO_*` names.
Nothing is required — defaults cover the standard local setup.

## Fresh data requirement

`/setup` only shows the admin-creation form when the panel data directory
has no administrator yet. First-run recordings require a new data
directory (convention: `./data-demo/` or a temporary directory).

- Never point a demo at `./data/` if that risks developer data.
- The script never deletes application data. If setup has already been
  completed it fails with: `Demo requires a fresh mcpanel data directory.`
- Any future cleanup helper must target demo-only directories and must never
  recursively delete a path taken directly from an unchecked env variable.

## What the demo does

`demo/first-run.ts` — "First Run → Create Server":

1. Opens `${url}/setup`, fails unless setup is required.
2. Creates the admin account (deterministic fills).
3. Logs in, waits for the dashboard (`New Server` button).
4. Opens New Server, creates a Paper server named `Survival` using the
   latest Minecraft version from the UI, verifies (not overrides) the
   application's Java selection (26.x ⇒ Java 25), accepts the EULA, submits.
5. Verifies the `Survival` card appears; holds the dashboard ~1.5 s in
   slow mode for a stable final frame.

The demo does not start the Minecraft server. Provisioning/startup belongs
in a future demo.

## Troubleshooting

- `Demo requires a fresh mcpanel data directory. Setup has already been
  completed.` — start mcpanel with an empty `--data-dir` (e.g. `./data-demo`).
- Playwright missing browser (`Executable doesn't exist`) — run
  `npx playwright install chromium`.
- `Minecraft versions failed to populate` — the panel backend or its
  catalog upstream is unreachable; check the server logs.
- `expected Java version 25 for Minecraft 26.x ...` — the application
  selected the wrong Java. This is an application bug the demo is designed
  to catch; do not paper over it in automation.
- `dashboard did not appear within 15 seconds` — login failed or the API
  is down; each phase reports its own stage name.

## Structure

```text
web/demo/
├── config.ts     # zero-config env/CLI parsing (nothing required)
├── browser.ts    # Playwright Chromium launch + video context
├── helpers.ts    # pacing, step(), condition-based waits (no raw sleeps)
├── recording.ts  # save the .webm to artifacts/demos/first-run.webm
├── first-run.ts  # First Run → Create Server workflow
└── README.md     # this file
```

Planned future scripts reuse these primitives:
`create-server`, `start-server`, `console`, `files`, `backups`, `playit`, `users`.
