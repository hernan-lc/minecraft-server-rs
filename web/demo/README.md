# mcpanel demo automation (Playwright)

Automated, reproducible mcpanel demo workflows using Playwright.
The first workflow drives the real web UI from first run to a genuinely
online Minecraft server and records the complete raw source footage.

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

`npm run demo:first-run` is human-readable by default and records the full
run to `artifacts/demos/first-run.webm` (1280×720), plus chapter markers in
`artifacts/demos/first-run.chapters.json` and sanitized startup diagnostics in
`artifacts/demos/first-run.diagnostics.json`. Provisioning is intentionally
uncut; long Java, Paper, or world-initialization sections are edited later.

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
[demo] startup 30s ui=preparing backend=preparing pid=unknown uptime=unknowns console=reconnecting logs=0 last=""
[demo] startup 60s ui=starting backend=starting pid=1234 uptime=58s console=connected logs=47 last="Preparing spawn area: 83%"
[demo] backend status: online
[demo] UI status: online
[demo] recording saved: .../artifacts/demos/first-run.webm
[demo] success
```

The video tells the story without narration: configure mcpanel → sign
in → create a Paper server → open it → start it → watch the real provisioning
finish online. A synthetic cursor (injected via `addInitScript`, never
touching production code) travels smoothly to each control with click
ripples, so every action is followable.

Note: the panel serves a strict `style-src 'self'` CSP, which would
silently block the injected cursor's stylesheet. The demo browser relaxes
`style-src` to `'unsafe-inline'` for document responses in its own
Playwright context only (`browser.ts` request routing); production headers
and all API/websocket traffic are untouched.

Fast mode only reduces presentation pauses; it still waits for real Online:

```bash
npm run demo:first-run:fast
```
To watch it live while it records:

```bash
npm run demo:first-run:headed
```

For a cold provisioning validation run:

```bash
npm run demo:first-run:cold
```

Cold mode does not create or inject application state. Use it with a newly
prepared panel data directory when uncached provisioning is required. The
default warm recording still uses fresh `/setup` state, but may benefit from
immutable host/backend downloads already available to the real provisioner.
Installing Java 25 on the demo host is the simplest warm optimization because
the panel's normal Java discovery can use it without demo-specific code.

## Options (all optional)

CLI flags (highest precedence):

```text
--slow / --fast         presentation pacing or test-suite speed
                        (default: slow; demo:* is watchable by default)
--headed / --headless   show the browser or not (default: headless)
--cold / --warm         cache-validation intent (default: warm)
--url <url>             panel URL
--user <name>           demo username (default: admin)
--password <pw>         demo password (default: mcpanel-demo-password)
```

Environment variables (or a `web/.env.demo` file, see
`.env.demo.example`) use the same values under `MCPANEL_DEMO_*` names. The
demo reports which `.env.demo` file it selected and never loads the generic
`.env` file implicitly. Existing process variables take precedence over that
file; CLI flags take precedence over both.
Timeout overrides are correctness limits, not presentation delays:

```dotenv
MCPANEL_DEMO_UI_TIMEOUT_MS=30000
MCPANEL_DEMO_CATALOG_TIMEOUT_MS=90000
MCPANEL_DEMO_CREATE_SERVER_TIMEOUT_MS=60000
MCPANEL_DEMO_START_TIMEOUT_MS=60000
MCPANEL_DEMO_ONLINE_TIMEOUT_MS=1200000
```

Nothing else is required — defaults cover the standard local setup.

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
7. Clicks the real Start control, accepts `preparing`, `starting`, or
   `online` as the initial acknowledgement, then keeps recording through the
   real provisioning (Java download, core download, first boot) until the
   status is `online` — this can take many minutes and is recorded uncut.
8. Holds the `online` server for a stable final frame.

Starting the server begins background provisioning (Java/JAR downloads)
inside the disposable demo data directory; the demo records it raw and does
not clean it up. Stop the panel and remove only the demo directory when done.

The demo never pre-creates an administrator, `Survival`, a world, or an
online process. Warm/cold concerns apply to immutable downloads only; the UI
workflow and Guardian lifecycle remain real. The current backend keeps
managed JDKs under its data directory and installs server artifacts into each
server directory. A shared verified Paper-artifact cache is a separate
backend follow-up, not a Playwright shortcut.

## Editing workflow

1. Record `first-run.webm`.
2. Inspect `first-run.chapters.json` for `setup`, `login`, `newServer`,
   `serverCreated`, `serverOpened`, `startClicked`, `startAccepted`, observed
   lifecycle states, `online`, and `end` (or `failed`).
3. Import the WebM into a video editor.
4. Trim or speed up long download/provisioning sections while preserving the
   Setup → Create → Start → Online narrative.
5. Export the edited demo separately.

Failed attempts preserve `first-run-failed.webm`,
`first-run-failed.png` when a screenshot is available, and
`first-run.chapters.json` plus `first-run-failed.diagnostics.json` for
diagnosis. The diagnostic artifact contains only effective non-secret config,
chapter events, sanitized server/log snapshots, and redacted network events;
it never stores passwords, cookies, CSRF values, authorization headers, or
WebSocket tickets.

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
- `Minecraft versions did not populate ... Last error: ...` — the panel
  backend or its catalog upstream is unreachable; the last visible catalog
  error is preserved in the demo output. Check the server logs.
- `expected Java version 25 for Minecraft 26.x ...` — the application
  selected the wrong Java. This is an application bug the demo is designed
  to catch; do not paper over it in automation.
- `dashboard did not appear within 30 seconds` — login failed or the API
  is down; each phase reports its own stage name.
- `Server crashed during first boot` — the demo fails immediately and includes
  UI/backend status, PID/uptime, progress stage/fraction when available,
  console transport state, recent Guardian/UI lines, and a likely failing
  layer. The backend REST snapshot is collected even when the browser console
  WebSocket is disconnected.
- `server did not enter preparing/starting/online ... after Start` —
  the Start action was rejected or provisioning stalled; check the panel
  logs and network access to Java/Minecraft upstreams.
- `Server did not reach online within 1200 seconds` — first provisioning
  needs to download Java (~150 MB) and the Paper build plus boot the
  server. The timeout is configurable; check the panel logs and retry.

## Structure

```text
web/demo/
├── config.ts     # env/CLI parsing, cache mode, and timeout categories
├── browser.ts    # Playwright Chromium launch + video context
├── cursor.ts     # injected demo cursor + human interaction helpers
├── helpers.ts    # pacing, step(), condition-based waits (no raw sleeps)
├── recording.ts  # save video, chapters, screenshots, and diagnostics
├── networkDiagnostics.ts # sanitized HTTP/WebSocket observation
├── first-run.ts  # First Run → Create Server → Start workflow
└── README.md     # this file
```

Planned future scripts reuse these primitives:
`create-server`, `start-server`, `console`, `files`, `backups`, `playit`, `users`.
