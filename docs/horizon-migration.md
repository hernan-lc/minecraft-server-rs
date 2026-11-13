# Horizon migration (in progress)

Guardian supports an experimental, host-managed Horizon 26.3 runtime. The panel
can create a Horizon server and initialize its configuration, but does not yet
install Horizon or prepare its game files. Existing Java server support is still present.

Build and install `horizon-server` and the headless engine from the pinned
`vendor/horizon` fork with:

```sh
./scripts/install-horizon.sh --data-dir ./data --resources /path/to/prepared/26.3
```

Run this from a recursive clone of the migration branch. A downloaded panel
release includes the installer but not Horizon source: clone the branch to build
it, or use `--from-build-dir` with binaries built on the host.

The script builds the pinned, clean submodule and validates both executables.
Use `--debug` for a debug build, or `--from-build-dir /path/to/target/debug`
to install trusted binaries already built for this host. `--resources` is
optional; when supplied, Horizon validates and copies the prepared game files
into a shared location under `data/horizon/.local/`. The platform sandbox mounts
that installation read-only when available. The script never
replaces an existing installation. Stop any running servers and manage updates
explicitly. The installation directory must be administrator-owned and outside
every writable server directory. The panel does not download these executables.

Prepare Horizon's game files from your own Minecraft installation as described
in `vendor/horizon/docs/hosting.md`. Supply the prepared directory to the
installer, or place a copy in an individual server's `resources/` directory.
The prepared game files come from your own Minecraft installation and must not
be committed or redistributed. Preparation may require Java; running the native
server does not. On first preparation or start, the panel runs Horizon's
initializer in a private staging directory. For an existing vanilla server, it
imports `server.properties` and leaves the original file and world in place;
back up the world before converting. Java plugins and mods cannot run in Horizon.

Select Horizon and version 26.3 when creating a server or changing its version.
The server directory is `<data-dir>/servers/<id>`. The API also accepts
`core: "horizon"`, `version: "26.3"`, no build, and an explicit EULA decision.
The panel's port is applied to `server.toml` on each start, Horizon's own panel
is disabled, and Horizon's internal crash restart is disabled so Guardian owns
restart policy. Other Horizon settings are retained. Existing Java heap and JVM
arguments do not limit or configure Horizon's native memory usage.

Horizon is started as `horizon-server run` and receives console commands and
`stop` over stdin. Linux and macOS use their platform sandbox when available;
Android and platforms without a supported sandbox require the existing explicit
`--allow-unsandboxed-servers` setting. Automated installation through the panel,
installation status, enforced resource limits, live engine metrics, and cross-platform
end-to-end validation remain unfinished. Guardian currently refuses to force-kill
a Horizon supervisor because the engine is a separate process. If a graceful stop
times out, use Horizon or host tools to stop both processes before restarting.
