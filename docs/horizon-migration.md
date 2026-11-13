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
Server settings report whether both programs pass filesystem checks; this does
not validate their compatibility or the prepared game files.

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

### Migrating a Paper or Spigot world

Paper and Spigot may store world data in a different layout from vanilla.
Horizon preparation stops when it detects the layouts below. To make a
converted copy, stop the Java server, create a new Horizon server in the panel
without starting it, and run:

```sh
python3 scripts/migrate-paper-world.py --source /path/to/java/server \
  --destination /path/to/panel/data/servers/NEW_SERVER_ID --world world
```

Use the exact `level-name` from `server.properties` for `--world`. The
destination must be new or empty. Keep both servers stopped throughout the copy;
the original remains intact. The script refuses symbolic links and existing
destination data, and leaves a marker that prevents Horizon from starting if a
copy fails. Inspect a partial destination before retrying. It preserves other
files from the old split-dimension directories under `.mcpanel-paper-originals`
in the new server. Java plugins and mods are copied but cannot run in Horizon.

For a manual conversion, stop the Java server and make a full backup before
changing anything. Substitute the configured `level-name` for `world` below.

For older Paper/Spigot worlds, copy `world_nether/DIM-1` to `world/DIM-1` and
`world_the_end/DIM1` to `world/DIM1`. If either destination exists, compare it
with the source and resolve the conflict using your backup; do not overwrite
it blindly. Preserve the original `world_nether` and `world_the_end` folders
outside the server directory after copying so Horizon can start.

For newer Paper worlds, move the following files, when present, from
`world/dimensions/minecraft/overworld/data/minecraft/` to
`world/data/minecraft/`: `game_rules.dat`, `scheduled_events.dat`,
`wandering_trader.dat`, `weather.dat`, and `world_gen_settings.dat`. Resolve
any existing destination files against the backup before replacing anything.
The panel does not validate plugin data or perform a complete compatibility
conversion. Refer to [Paper's migration guide](https://docs.papermc.io/paper/migration/)
for the source layouts and additional limitations.

When the operating system exposes the relevant processes, the panel reports
CPU and the summed resident memory of the supervisor, engine and descendants.
Shared pages may be counted more than once. Metrics are unavailable when the
engine cannot be identified; CPU can also be unavailable separately.
On Android, when host CPU counters are restricted, CPU is estimated from the
visible processes' tick deltas over elapsed time. A new or changed process tree
needs another sample before its CPU usage can be shown.

Horizon is started as `horizon-server run` and receives console commands and
`stop` over stdin. Linux and macOS use their platform sandbox when available;
Android and platforms without a supported sandbox require the existing explicit
`--allow-unsandboxed-servers` setting. Automated installation through the panel,
full installation validation, enforced resource limits, and cross-platform
end-to-end validation remain unfinished. The pinned Horizon fork advertises a
force-stop control protocol: the supervisor kills and reaps its engine before
exiting. Older binaries without this protocol cannot be force-stopped by the
panel; use Horizon or host tools if their graceful stop times out.

To run the isolated native integration test with locally built programs and
your prepared game files, set `HORIZON_SMOKE_SERVER`, `HORIZON_SMOKE_ENGINE`,
and `HORIZON_SMOKE_RESOURCES` to their absolute paths, then run:

```sh
HORIZON_SMOKE_FORCE_STOP=1 cargo test -p guardian --test supervisor \
  real_horizon_starts_and_stops -- --ignored --nocapture
```

The test creates a temporary server world and checks the force-stop protocol;
on Linux and Android it also checks that the engine process was reaped. Omit
`HORIZON_SMOKE_FORCE_STOP` to test a graceful stop instead.
