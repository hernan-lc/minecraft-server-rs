# Horizon migration (in progress)

Guardian supports an experimental, host-managed Horizon 26.3 runtime. The panel
does not yet install Horizon, prepare its game files, initialize its configuration,
or offer it in the setup UI. Existing Java server support is still present.

Build `horizon-server` and the headless `horizon-client` from the pinned
`vendor/horizon` fork following its `packaging/server/Dockerfile` and hosting
guide. Place them in the panel data directory as `horizon/horizon-server` and
`horizon/horizon-engine` (add `.exe` on Windows). This directory must be
administrator-owned and outside every writable server directory. The panel
does not download or update these executables.

Prepare Horizon's game files from your own Minecraft installation as described
in `vendor/horizon/docs/hosting.md`, and place the prepared files in the
server's `resources/` directory. Preparation may require Java; running the
native server does not. Use Horizon's `init` in the server directory to create
`server.toml`. For an existing vanilla server, Horizon imports
`server.properties`; back up the world before converting. Java plugins and
mods cannot run in Horizon.

Create or update a server record through the panel API with `core: "horizon"`,
`version: "26.3"`, no build, and an explicit EULA decision. The server directory
is `<data-dir>/servers/<id>`; initialize that directory before starting it.
The panel's port is applied to `server.toml` on each start, Horizon's own panel
is disabled, and Horizon's internal crash restart is disabled so Guardian owns
restart policy. Other Horizon settings are retained. Existing Java heap and JVM
arguments do not limit or configure Horizon's native memory usage.

Horizon is started as `horizon-server run` and receives console commands and
`stop` over stdin. Linux and macOS use their platform sandbox when available;
Android and platforms without a supported sandbox require the existing explicit
`--allow-unsandboxed-servers` setting. Native provisioning, installation status,
the catalog, UI, resource limits, live engine metrics, and cross-platform
end-to-end validation remain unfinished. Guardian currently refuses to force-kill
a Horizon supervisor because the engine is a separate process. If a graceful stop
times out, use Horizon or host tools to stop both processes before restarting.
