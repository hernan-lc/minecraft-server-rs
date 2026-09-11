# mcpanel demo automation (Playwright)

Automated, reproducible mcpanel demo workflows using Playwright.
The first workflow drives the real web UI from first run to a started
server and records it to video — with zero configuration.

## Purpose

- Reproducible automated demos of the real mcpanel UI, recorded raw and
  uncut for manual editing.
- A foundation for future browser workflow/regression testing.
- Fully deterministic: stable `data-testid` locators, no AI, no API keys,
  no cloud sessions.

The password is filled directly with a locator and never logged.

## Requirements

- Node/npm.
- A running mcpanel serving the UI (default `http://127.0.0.1:8080`).
- Playwright's Chromium: `npx playwright install chromium` (one time).

## Recording a demo

No environment setup needed. `npm run demo:first-run` is human-readable
by default and records the full run to `artifacts/demos/first-run.webm`
(1280×720), plus chapter markers in `artifacts/demos/first-run.chapters.json`
(`startClicked` / `preparing` / `online` / `end`, in video-relative seconds)
for trimming the footage afterwards.

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
[demo] opening Survival
[demo] starting server
[demo] status: preparing
[demo] recording saved: .../artifacts/demos/first-run.webm
[demo] success
```

The video tells the story without narration: configure mcpanel → sign
in → create a Paper server → open it → start it → mcpanel begins
provisioning it. A synthetic cursor (injected via `addInitScript`, never
touching production code) travels smoothly to each control with click
ripples, so every action is followable.

Note: the panel serves a strict `style-src 'self'` CSP, which would
silently block the injected cursor's stylesheet. The demo browser relaxes
`style-src` to `'unsafe-inline'` for document responses in its own
Playwright context only (`browser.ts` request routing); production headers
and all API/websocket traffic are untouched.

Fast mode is the special case for development/debugging:

```bash
npm run demo:first-run:fast
```
To watch it live while it records:

```bash
npm run demo:first-run:headed
```

## Options (all optional)

CLI flags (highest precedence):

```text
--slow / --fast         presentation pacing or test-suite speed
                        (default: slow; demo:* is watchable by default)
--headed / --headless   show the browser or not (default: headless)
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

`demo/first-run.ts` — "First Run → Create Server → Start":

1. Opens `${url}/setup`, fails unless setup is required.
2. Creates the admin account (typed username, filled password).
3. Logs in, waits for the dashboard (`New Server` button).
4. Opens New Server, creates a Paper server named `Survival` using the
   latest Minecraft version from the UI, verifies (not overrides) the
   application's Java selection (26.x ⇒ Java 25), accepts the EULA, submits.
5. Verifies the `Survival` card appears and holds it on screen.
6. Opens `Survival`, waits for the server detail view.
7. Clicks the real Start control, waits for `preparing`, then keeps
   recording through the real provisioning (Java download, core download,
   first boot) until the status is `online` — this takes minutes on first
   start and is recorded uncut. Chapter markers locate each segment.
8. Holds the running server for a stable final frame.

Starting the server begins background provisioning (Java/JAR downloads)
inside the disposable demo data directory; the demo records it raw and
does not clean it up. Stop the panel and delete `data-demo/` when done.

## Waiting vs pacing

Two concerns stay separate throughout `web/demo/`:

- Application synchronization: `waitFor()`, `waitForFunction()`, UI state.
- Presentation pacing: `demoPause()`, cursor movement, typing delay, holds.

Presentation pauses always run *after* the expected state exists — never
as a substitute for waiting on it.

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
- `server did not enter preparing/starting/running ... after Start` —
  the Start action was rejected or provisioning stalled; check the panel
  logs and network access to Java/Minecraft upstreams.
- `server did not reach online within 15 minutes` — first provisioning
  needs to download Java (~150 MB) and the Paper build plus boot the
  server; on a slow line or with blocked upstreams it can exceed the
  timeout. Check the panel logs and retry.

## Structure

```text
web/demo/
├── config.ts     # zero-config env/CLI parsing (slow by default)
├── browser.ts    # Playwright Chromium launch + video context
├── cursor.ts     # injected demo cursor + human interaction helpers
├── helpers.ts    # pacing, step(), condition-based waits (no raw sleeps)
├── recording.ts  # save the .webm to artifacts/demos/first-run.webm
├── first-run.ts  # First Run → Create Server → Start workflow
└── README.md     # this file
```

Planned future scripts reuse these primitives:
`create-server`, `start-server`, `console`, `files`, `backups`, `playit`, `users`.
