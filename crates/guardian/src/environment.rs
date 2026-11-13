//! Turning a [`ServerConfig`] into something that can actually be executed.
//!
//! This is the only module that talks to both `java-path` and `minecraft-core`.
//! It answers one question: *given this config, what is the java binary, what
//! is the jar, and is the directory ready?*

use crate::config::ServerConfig;
use crate::error::{Error, Result};
use crate::fs::ScopedFs;
use crate::install::Installation;
use crate::java::JavaResolver;
use java_path::{JavaInstallation, JavaInstaller, SelectExt};
use minecraft_core::MinecraftClient;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// A prepared, runnable server.
///
/// Every path here is absolute, and that is load-bearing rather than cosmetic:
/// the JVM is spawned with its working directory set to [`Self::directory`], so
/// a path relative to the panel's own cwd would be re-resolved against the
/// server directory and silently fail to launch.
#[derive(Debug, Clone)]
pub struct ServerEnvironment {
    /// The executable and artifacts required by this server.
    pub runtime: ServerRuntime,
    /// Working directory for the process.
    pub directory: PathBuf,
}

/// A prepared Java or native Horizon server runtime.
#[derive(Debug, Clone)]
pub enum ServerRuntime {
    /// A Java server JAR and the JVM that runs it.
    Java {
        /// Absolute path to the Java launcher.
        java: PathBuf,
        /// Java feature version selected during provisioning.
        java_major: u32,
        /// Absolute path to the server JAR.
        jar: PathBuf,
    },
    /// A host-managed Horizon installation containing supervisor and engine.
    Horizon {
        /// Directory containing `horizon-server` and `horizon-engine`.
        installation: PathBuf,
    },
}

/// Resolve `path` against the current working directory when it is relative.
///
/// `canonicalize` is deliberately not used: it requires the path to exist and
/// resolves symlinks, neither of which is wanted for a jar that is about to be
/// downloaded or a JDK reached through a symlinked home.
pub(crate) fn absolute(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        // With no cwd there is nothing better to do than leave it alone.
        Err(_) => path,
    }
}

impl ServerEnvironment {
    /// Rewrite every relative path as an absolute one.
    pub fn absolutize(mut self) -> Self {
        match &mut self.runtime {
            ServerRuntime::Java { java, jar, .. } => {
                *java = absolute(std::mem::take(java));
                *jar = absolute(std::mem::take(jar));
            }
            ServerRuntime::Horizon { installation } => {
                *installation = absolute(std::mem::take(installation));
            }
        }
        self.directory = absolute(self.directory);
        self
    }
}

/// Where downloaded JDKs are cached, shared across every server on the host.
fn jdk_root(data_dir: &Path) -> PathBuf {
    data_dir.join("jdks")
}

/// Find a local Java matching `major`, installing a Temurin build if none exists.
///
/// Local installations are preferred: downloading a JDK is slow, and a machine
/// that already has the right major version does not need another copy.
///
/// Discovery goes through an ephemeral [`JavaResolver`]; callers that already
/// own the panel's shared resolver should use [`resolve_java_with`] so the
/// cached pass is reused instead of scanning the machine again.
pub async fn resolve_java(
    major: u32,
    data_dir: &Path,
    on_progress: impl Fn(String, Option<f32>) + Send + Sync + Clone + 'static,
) -> Result<JavaInstallation> {
    let resolver = JavaResolver::new(data_dir);
    resolve_java_with(major, data_dir, &resolver, on_progress).await
}

/// [`resolve_java`] over an explicit [`JavaResolver`].
///
/// This is the provisioning path the panel shares with the Java catalog: both
/// read through the same cached discovery pass, so a catalog refresh followed
/// by a server install scans the machine once, not twice.
pub async fn resolve_java_with(
    major: u32,
    data_dir: &Path,
    resolver: &JavaResolver,
    on_progress: impl Fn(String, Option<f32>) + Send + Sync + Clone + 'static,
) -> Result<JavaInstallation> {
    resolve_java_on(
        major,
        data_dir,
        resolver,
        java_path::Platform::current(),
        on_progress,
    )
    .await
}

async fn resolve_java_on(
    major: u32,
    data_dir: &Path,
    resolver: &JavaResolver,
    platform: java_path::Platform,
    on_progress: impl Fn(String, Option<f32>) + Send + Sync + Clone + 'static,
) -> Result<JavaInstallation> {
    if let Ok(installs) = resolver.javas() {
        if let Ok(found) = installs.select().major(major).current_arch().best() {
            return Ok(found.clone());
        }
    }

    // Generic Linux Temurin archives require glibc and cannot execute in
    // Android's Bionic environment. Package installation stays operator-owned.
    if platform == java_path::Platform::Termux {
        return Err(Error::TermuxJavaRequired(major));
    }

    on_progress(format!("installing Java {major}"), None);

    let dir = jdk_root(data_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| Error::io(&dir, e))?;

    let progress = on_progress.clone();
    JavaInstaller::adoptium()
        .version(major)
        .install_dir(&dir)
        .cache_dir(dir.join(".cache"))
        .on_event(move |event| match event {
            java_path::InstallEvent::Resolving => progress(format!("resolving Java {major}"), None),
            java_path::InstallEvent::Downloading { downloaded, total } => {
                let fraction = total
                    .filter(|t| *t > 0)
                    .map(|t| downloaded as f32 / t as f32);
                progress(format!("downloading Java {major}"), fraction);
            }
            java_path::InstallEvent::Verifying => progress(format!("verifying Java {major}"), None),
            java_path::InstallEvent::Extracting => {
                progress(format!("extracting Java {major}"), None)
            }
            java_path::InstallEvent::Installed { .. } => {
                progress(format!("installed Java {major}"), Some(1.0))
            }
            _ => {}
        })
        .install()
        .await
        .map_err(|e| Error::JavaUnavailable(major, e.to_string()))
}

/// Build the semantic progress label for a server-jar download.
fn download_stage(core: &str, version: &str) -> String {
    format!("downloading {core} {version}")
}

/// Download the server jar for `config`, returning it and the build it resolved to.
pub async fn resolve_jar(
    config: &ServerConfig,
    on_progress: impl Fn(String, Option<f32>) + Send + Sync + Clone + 'static,
) -> Result<(PathBuf, String)> {
    // Stream download progress back to the guardian's Progress event.
    let progress_for_download = on_progress.clone();
    let core_for_progress = config.core.clone();
    let version_for_progress = config.version.clone();
    let client = MinecraftClient::builder()
        .user_agent("minecraft-server-rs/0.1 (https://github.com/hernan-lc/minecraft-server-rs)")
        .on_progress(std::sync::Arc::new(move |p: minecraft_core::Progress| {
            let fraction = p
                .total
                .filter(|t| *t > 0)
                .map(|t| p.downloaded as f32 / t as f32);
            // Keep the stage semantic. The presentation layer owns percentage
            // formatting; putting it here makes structured and rendered
            // progress display the same percentage twice.
            progress_for_download(
                download_stage(&core_for_progress, &version_for_progress),
                fraction,
            );
        }))
        .build()?;

    on_progress(
        format!("resolving {} {}", config.core, config.version),
        None,
    );
    let build = match &config.build {
        Some(id) => client.build(&config.core, &config.version, id).await?,
        None => client.latest_build(&config.core, &config.version).await?,
    };
    ensure_binary_artifact(&config.core, build.artifact.kind)?;

    let fs = ScopedFs::open(&config.directory).map_err(|e| Error::io(&config.directory, e))?;
    // The vendor downloader is streaming and checksum-verifying, but its
    // portable path API cannot promise no-follow semantics for an attacker
    // replacing the destination concurrently. Publish into the open
    // capability after validation instead.
    if let Ok(metadata) = fs.metadata("server.jar") {
        if metadata.is_symlink {
            fs.remove("server.jar")
                .map_err(|e| Error::io(config.directory.join("server.jar"), e))?;
        }
    }
    let temporary_name = format!(".mcpanel-server-{}.jar", Uuid::new_v4().simple());
    let temporary = config.directory.join(&temporary_name);
    on_progress(
        format!(
            "downloading {} {} build {}",
            config.core, config.version, build.build_id
        ),
        None,
    );

    // `verify` makes the download checksum-checked; `force` is off so an
    // already-correct jar is reused rather than re-fetched.
    let downloaded = client.download(&build).to(&temporary).verify(true).await;
    if let Err(error) = downloaded {
        let _ = fs.remove(&temporary_name);
        return Err(error.into());
    }
    if let Err(error) = fs.replace_file(&temporary_name, "server.jar") {
        let _ = fs.remove(&temporary_name);
        return Err(Error::io(config.directory.join("server.jar"), error));
    }

    let jar = config.directory.join("server.jar");

    Ok((jar, build.build_id.to_string()))
}

/// Create the server directory and write the files the server refuses to start without.
pub async fn scaffold(config: &ServerConfig) -> Result<()> {
    let dir = &config.directory;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| Error::io(dir, e))?;
    let fs = ScopedFs::open(dir).map_err(|e| Error::io(dir, e))?;

    if config.eula_accepted {
        let body = "# Accepted through the panel.\neula=true\n";
        fs.write_atomic("eula.txt", body.as_bytes())
            .map_err(|e| Error::io(dir.join("eula.txt"), e))?;
    }

    // server.properties is seeded once and then left to the operator, except for
    // the port: the panel presents that as a setting, so it has to be the one
    // that wins. Every other line is preserved exactly as edited.
    let props = dir.join("server.properties");
    let existing = fs
        .read_file("server.properties")
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok());

    let body = match existing {
        Some(text) => set_property(&text, "server-port", &config.port.to_string()),
        None => format!(
            "server-port={}\nmotd=A Minecraft Server\nmax-players=20\nonline-mode=true\n",
            config.port
        ),
    };

    fs.write_atomic("server.properties", body.as_bytes())
        .map_err(|e| Error::io(&props, e))?;

    Ok(())
}

/// Set `key` to `value` in a `.properties` document, preserving everything else.
///
/// Comments, ordering and unrelated keys survive untouched, because the file is
/// also edited by hand through the panel's file manager.
pub(crate) fn set_property(text: &str, key: &str, value: &str) -> String {
    let mut out = String::with_capacity(text.len() + key.len() + value.len() + 2);
    let mut replaced = false;

    for line in text.lines() {
        let is_target = line
            .split_once('=')
            .is_some_and(|(name, _)| name.trim() == key)
            && !line.trim_start().starts_with('#');

        if is_target && !replaced {
            out.push_str(&format!("{key}={value}"));
            replaced = true;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }

    if !replaced {
        out.push_str(&format!("{key}={value}\n"));
    }

    out
}

/// Whether an existing installation may be reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provision {
    /// Reuse what is installed when it already satisfies the config.
    IfNeeded,
    /// Re-resolve and download regardless — how a deliberate update is done.
    Force,
}

/// Reject installer artifacts before they can be published as `server.jar`.
///
/// A Forge build resolves to an *installer* jar: executing it with
/// `java -jar server.jar` does not start a server, it runs installer
/// processors (more child processes, more consoles, no Minecraft). Until an
/// explicit installer-provisioning flow exists, fail loudly here rather than
/// shipping a server that can never come online.
pub(crate) fn ensure_binary_artifact(core: &str, kind: minecraft_core::ArtifactKind) -> Result<()> {
    if kind == minecraft_core::ArtifactKind::Installer {
        return Err(Error::UnsupportedInstaller {
            core: core.to_string(),
        });
    }
    Ok(())
}

/// The whole provisioning flow: directory, EULA, Java, jar.
///
/// With [`Provision::IfNeeded`] and a matching installation this touches no
/// network at all, so a restart is as fast as launching the JVM.
///
/// Provisioning discovery goes through an ephemeral [`JavaResolver`]; callers
/// that already own the panel's shared resolver should use
/// [`prepare_with_resolver`] so the cached pass is reused.
pub async fn prepare(
    config: &ServerConfig,
    data_dir: &Path,
    mode: Provision,
    on_progress: impl Fn(String, Option<f32>) + Send + Sync + Clone + 'static,
) -> Result<ServerEnvironment> {
    let resolver = JavaResolver::new(data_dir);
    prepare_with_resolver(config, data_dir, mode, &resolver, on_progress).await
}

/// [`prepare`] over an explicit [`JavaResolver`].
///
/// This is the Start path: with a trusted installation on disk it performs
/// zero discovery passes and zero subprocess probes — see
/// [`trusted_installation`] — and launches exactly the recorded JVM.
pub async fn prepare_with_resolver(
    config: &ServerConfig,
    data_dir: &Path,
    mode: Provision,
    resolver: &JavaResolver,
    on_progress: impl Fn(String, Option<f32>) + Send + Sync + Clone + 'static,
) -> Result<ServerEnvironment> {
    prepare_with_resolver_and_sandbox(
        config,
        data_dir,
        mode,
        resolver,
        crate::sandbox::SandboxPolicy::default(),
        on_progress,
    )
    .await
}

pub(crate) async fn prepare_with_resolver_and_sandbox(
    config: &ServerConfig,
    data_dir: &Path,
    mode: Provision,
    resolver: &JavaResolver,
    policy: crate::sandbox::SandboxPolicy,
    on_progress: impl Fn(String, Option<f32>) + Send + Sync + Clone + 'static,
) -> Result<ServerEnvironment> {
    if !config.eula_accepted {
        return Err(Error::EulaNotAccepted);
    }

    if config.core.eq_ignore_ascii_case("horizon") {
        if mode == Provision::Force {
            return Err(Error::InvalidConfiguration(
                "update the host-managed Horizon installation outside the panel".into(),
            ));
        }
        return prepare_horizon(config, data_dir, policy, on_progress).await;
    }

    // Cheap, and it keeps eula.txt and the port in step with the config.
    scaffold(config).await?;

    let installed = Installation::load(&config.directory).await;

    if mode == Provision::IfNeeded {
        if let Some(installation) = &installed {
            match installation.mismatch(config) {
                None if trusted_installation(installation, config, data_dir) => {
                    return Ok(ServerEnvironment {
                        runtime: ServerRuntime::Java {
                            java: installation.java.clone(),
                            java_major: installation.java_major,
                            jar: installation.jar.clone(),
                        },
                        directory: config.directory.clone(),
                    }
                    .absolutize())
                }
                None => on_progress(
                    "reinstalling: installation metadata is not trusted".into(),
                    None,
                ),
                // Say what changed, so a surprise download is never unexplained.
                Some(reason) => on_progress(format!("reinstalling: {reason}"), None),
            }
        }
    }

    let p = on_progress.clone();
    let java = resolve_java_with(config.java_major, data_dir, resolver, p).await?;

    let p = on_progress.clone();
    let (jar, build) = resolve_jar(config, p).await?;

    let environment = ServerEnvironment {
        runtime: ServerRuntime::Java {
            java: absolute(java.java.clone()),
            java_major: java.major(),
            jar: absolute(jar),
        },
        directory: config.directory.clone(),
    }
    .absolutize();

    Installation {
        core: config.core.clone(),
        version: config.version.clone(),
        // Recorded so an unpinned "latest" stops moving after the first install.
        build,
        java_major: java.major(),
        java: absolute(java.java.clone()),
        // The trust anchor for warm Starts: the launcher must live at exactly
        // this home's `bin/java[.exe]`.
        java_home: java.home.clone(),
        jar: match &environment.runtime {
            ServerRuntime::Java { jar, .. } => jar.clone(),
            ServerRuntime::Horizon { .. } => unreachable!(),
        },
        installed_at: now_rfc3339(),
    }
    .save(&config.directory)
    .await?;

    Ok(environment)
}

/// Prepare a Horizon server without resolving Java.
/// The pinned fork targets 26.3. An administrator installs both native
/// executables under the panel data directory; Horizon itself imports existing
/// vanilla settings in a private staging directory on first preparation.
pub(crate) async fn prepare_horizon(
    config: &ServerConfig,
    data_dir: &Path,
    policy: crate::sandbox::SandboxPolicy,
    on_progress: impl Fn(String, Option<f32>),
) -> Result<ServerEnvironment> {
    if !config.eula_accepted {
        return Err(Error::EulaNotAccepted);
    }
    if config.version != crate::config::HORIZON_VERSION || config.build.is_some() {
        return Err(Error::InvalidConfiguration(format!(
            "the pinned Horizon runtime supports version {} without a build selector",
            crate::config::HORIZON_VERSION
        )));
    }
    let installation = absolute(data_dir.join("horizon"));
    let directory = absolute(config.directory.clone());
    // This also rejects missing executables, unsafe layouts and symlink escapes.
    crate::sandbox::horizon_command(&installation, &directory, policy).map_err(|error| {
        match error {
            Error::Io { ref source, .. } if source.kind() == std::io::ErrorKind::NotFound => Error::InvalidConfiguration("Horizon installation is missing; place horizon-server and horizon-engine in the panel data directory's horizon folder".into()),
            other => other,
        }
    })?;
    let fs = ScopedFs::open(&directory).map_err(|error| Error::io(&directory, error))?;
    let migration_marker = ".mcpanel-migration-in-progress";
    match fs.metadata(migration_marker) {
        Ok(_) => return Err(Error::InvalidConfiguration(
            "a Paper world migration is incomplete; inspect the server directory before starting"
                .into(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(Error::io(directory.join(migration_marker), error)),
    }
    let path = directory.join("server.toml");
    let bytes = match fs.read_file("server.toml") {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            on_progress("initializing Horizon configuration".into(), None);
            initialize_horizon(
                &fs,
                &directory,
                &installation,
                data_dir,
                config.port,
                policy,
            )
            .await?
        }
        Err(error) => return Err(Error::io(&path, error)),
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        Error::InvalidConfiguration("Horizon server.toml must be valid UTF-8".into())
    })?;
    let mut document = text.parse::<toml_edit::DocumentMut>().map_err(|error| {
        Error::InvalidConfiguration(format!("invalid Horizon server.toml: {error}"))
    })?;
    check_migrated_world(&fs, &directory, &document)?;
    check_paper_world_layout(&fs, &directory, &document)?;
    for (section, key, value) in [
        ("server", "port", toml_edit::value(i64::from(config.port))),
        ("engine", "restart_on_crash", toml_edit::value(false)),
        ("panel", "bind", toml_edit::value("")),
    ] {
        let section = document.entry(section).or_insert(toml_edit::table());
        let table = section.as_table_like_mut().ok_or_else(|| {
            Error::InvalidConfiguration("Horizon settings sections must be tables".into())
        })?;
        table.insert(key, value);
    }
    fs.write_atomic("server.toml", document.to_string().as_bytes())
        .map_err(|error| Error::io(&path, error))?;
    fs.write_atomic("eula.txt", b"# Accepted through the panel.\neula=true\n")
        .map_err(|error| Error::io(directory.join("eula.txt"), error))?;
    on_progress("Horizon installation ready".into(), Some(1.0));
    Ok(ServerEnvironment {
        runtime: ServerRuntime::Horizon { installation },
        directory,
    })
}

/// Read the configured world name, or Horizon's default when omitted.
fn configured_world(document: &toml_edit::DocumentMut) -> Result<&str> {
    let world = match document.get("world").and_then(|table| table.get("name")) {
        None => "world",
        Some(item) => item.as_str().ok_or_else(|| {
            Error::InvalidConfiguration("Horizon world.name must be a string".into())
        })?,
    };
    if world.is_empty() || world.contains(['/', '\\']) || world == "." || world == ".." {
        return Err(Error::InvalidConfiguration(
            "Horizon world.name must be a folder name".into(),
        ));
    }
    Ok(world)
}

/// A converted copy must start the same world that was converted.
fn check_migrated_world(
    fs: &ScopedFs,
    directory: &Path,
    document: &toml_edit::DocumentMut,
) -> Result<()> {
    use std::io::Read;
    #[derive(serde::Deserialize)]
    struct Manifest {
        version: u32,
        world: String,
    }
    let path = ".mcpanel-migrated-world.json";
    match fs.metadata(path) {
        Ok(metadata) if metadata.is_file && !metadata.is_symlink => {}
        Ok(_) => {
            return Err(Error::InvalidConfiguration(
                "migration world record must be a regular file".into(),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(Error::io(directory.join(path), error)),
    }
    let file = match fs.open_file(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(Error::io(directory.join(path), error)),
    };
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|error| Error::io(directory.join(path), error))?;
    if bytes.len() > 4096 {
        return Err(Error::InvalidConfiguration(
            "migration world record exceeds 4 KiB".into(),
        ));
    }
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|_| Error::InvalidConfiguration("invalid migration world record".into()))?;
    if manifest.version != 1 {
        return Err(Error::InvalidConfiguration(
            "unsupported migration world record version".into(),
        ));
    }
    let configured = configured_world(document)?;
    if manifest.world != configured {
        return Err(Error::InvalidConfiguration(format!(
            "converted world {:?} does not match configured world {:?}; correct server.toml or server.properties before starting",
            manifest.world, configured
        )));
    }
    let level = std::path::Path::new(configured).join("level.dat");
    match fs.metadata(&level) {
        Ok(metadata) if metadata.is_file && !metadata.is_symlink => {}
        Ok(_) => {
            return Err(Error::InvalidConfiguration(
                "converted world level.dat is not a regular file".into(),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::InvalidConfiguration(
                "converted world level.dat is missing".into(),
            ))
        }
        Err(error) => return Err(Error::io(directory.join(&level), error)),
    }
    Ok(())
}

/// Paper and Spigot can put dimensions where the vanilla-layout engine will
/// not find them. Refuse to silently create empty dimensions during migration.
fn check_paper_world_layout(
    fs: &ScopedFs,
    directory: &Path,
    document: &toml_edit::DocumentMut,
) -> Result<()> {
    let world = configured_world(document)?;
    let exists = |path: &std::path::Path| -> Result<bool> {
        match fs.metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(Error::io(directory.join(path), error)),
        }
    };
    for (suffix, dimension) in [("_nether", "DIM-1"), ("_the_end", "DIM1")] {
        let source = std::path::PathBuf::from(format!("{world}{suffix}")).join(dimension);
        if exists(&source)? {
            return Err(Error::InvalidConfiguration(format!(
                "Paper/Spigot dimension {} is separate from {world}; migrate it before starting Horizon (see docs/horizon-migration.md)",
                source.display()
            )));
        }
    }
    let modern = std::path::Path::new(world).join("dimensions/minecraft/overworld/data/minecraft");
    for file in [
        "game_rules.dat",
        "scheduled_events.dat",
        "wandering_trader.dat",
        "weather.dat",
        "world_gen_settings.dat",
    ] {
        if exists(&modern.join(file))? {
            return Err(Error::InvalidConfiguration(format!(
                "Paper world data {} is not in its vanilla location; migrate it before starting Horizon (see docs/horizon-migration.md)",
                modern.join(file).display()
            )));
        }
    }
    Ok(())
}

/// Run Horizon's importer in a private scratch directory, so it cannot modify
/// an existing world or replace operator files while translating properties.
async fn initialize_horizon(
    server_fs: &ScopedFs,
    directory: &Path,
    installation: &Path,
    data_dir: &Path,
    port: u16,
    policy: crate::sandbox::SandboxPolicy,
) -> Result<Vec<u8>> {
    use std::io::Read;
    let scratch = tempfile::Builder::new()
        .prefix(".horizon-init-")
        .tempdir_in(data_dir)
        .map_err(|error| Error::io(data_dir, error))?;
    match server_fs.open_file("server.properties") {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(1_048_577)
                .read_to_end(&mut bytes)
                .map_err(|error| Error::io(directory.join("server.properties"), error))?;
            if bytes.len() > 1_048_576 {
                return Err(Error::InvalidConfiguration(
                    "server.properties exceeds 1 MiB".into(),
                ));
            }
            std::fs::write(scratch.path().join("server.properties"), bytes)
                .map_err(|error| Error::io(scratch.path().join("server.properties"), error))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(Error::io(directory.join("server.properties"), error)),
    }
    let (mut command, _) =
        crate::sandbox::horizon_init_command(installation, scratch.path(), port, policy)?;
    command
        .env_clear()
        .envs(crate::process::sanitized_environment(std::env::vars_os()))
        .env("HOME", scratch.path())
        .env("USERPROFILE", scratch.path())
        .env("TMPDIR", scratch.path())
        .env("TMP", scratch.path())
        .env("TEMP", scratch.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(std::time::Duration::from_secs(60), command.status())
        .await
        .map_err(|_| Error::Task("Horizon initialization timed out".into()))?
        .map_err(|error| Error::io(installation.join("horizon-server"), error))?;
    if !status.success() {
        return Err(Error::InvalidConfiguration(
            "Horizon initialization failed; check the installation and server.properties".into(),
        ));
    }
    std::fs::read(scratch.path().join("server.toml"))
        .map_err(|error| Error::io(scratch.path().join("server.toml"), error))
}

/// Installation metadata lives inside a server directory an operator can
/// edit. Only reuse a jar at the exact managed destination and a Java launcher
/// that is bound to its recorded Java home. This prevents metadata from
/// turning `start` into a general executable launcher.
///
/// Deliberately subprocess-free: no `java_path::discover()`, no `reg.exe`,
/// no `java -version` probes. A warm Start performs only filesystem reads
/// (canonicalization, metadata, the JDK `release` file), so restarting a
/// provisioned server spawns exactly one process — the Minecraft JVM.
///
/// Trust rules, in order:
/// 1. the recorded launcher must be exactly `<recorded home>/bin/java[.exe]`
///    (canonical comparison), which rejects `C:\malicious.exe` outright;
/// 2. it must be a regular file that still exists;
/// 3. it must not live inside the server directory itself, where an attacker
///    able to edit the metadata could also drop a file;
/// 4. a launcher under the panel's managed `data/jdks` cache is accepted on
///    the strength of that root plus the recorded major matching the config;
/// 5. any other (system) launcher is accepted only when the JDK `release`
///    file at its recorded home still parses — with no subprocess and no
///    heuristics — to the configured major version.
fn trusted_installation(
    installation: &Installation,
    config: &ServerConfig,
    data_dir: &Path,
) -> bool {
    let expected_jar = absolute(config.directory.clone()).join("server.jar");
    if absolute(installation.jar.clone()) != expected_jar {
        return false;
    }
    if !std::fs::symlink_metadata(&expected_jar)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
    {
        return false;
    }

    let Some(java) = std::fs::canonicalize(&installation.java).ok() else {
        return false;
    };
    if !java.is_file() {
        return false;
    }

    // Records written before `java_home` existed derive the home from the
    // launcher layout (`<home>/bin/java[.exe]`); new records carry it
    // explicitly. Either way the binding below is enforced identically.
    let recorded_home = if installation.java_home.as_os_str().is_empty() {
        java.parent()
            .and_then(|bin| bin.parent())
            .map(Path::to_path_buf)
    } else {
        Some(installation.java_home.clone())
    };
    let Some(recorded_home) = recorded_home else {
        return false;
    };
    let Ok(home) = std::fs::canonicalize(&recorded_home) else {
        return false;
    };
    if java != home.join("bin").join(java_exe_name()) {
        return false;
    }

    // The server directory is operator-writable through the file manager; a
    // launcher inside it is attacker-placed by definition.
    let server_dir = std::fs::canonicalize(&config.directory)
        .unwrap_or_else(|_| absolute(config.directory.clone()));
    if java.starts_with(&server_dir) {
        return false;
    }

    if installation.java_major != config.java_major {
        return false;
    }

    let downloaded_root = std::fs::canonicalize(jdk_root(data_dir)).ok();
    if downloaded_root
        .as_ref()
        .is_some_and(|root| java.starts_with(root))
    {
        return true;
    }

    // System Java: re-read the authoritative `release` file locally. This
    // runs no executable and performs no discovery; it only confirms the
    // home on disk still is the major version the install was provisioned
    // for. Anything else (missing/unparsable file, version drift, a home
    // swapped for another) falls through to a full reinstall.
    java_path::inspect_java_home_with(
        &home,
        java_path::InspectOptions::strict(),
        java_path::DiscoverySource::Registry,
    )
    .ok()
    .is_some_and(|install| install.major() == config.java_major)
}

/// The launcher file name `trusted_installation` binds homes to.
///
/// Mirrors `java-path`'s layout: `java.exe` on Windows, `java` elsewhere.
fn java_exe_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "java.exe"
    } else {
        "java"
    }
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_android_java_reports_package_install_without_creating_cache() {
        use crate::java::CountingDiscovery;
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let resolver = JavaResolver::with_backend(&data, Arc::new(CountingDiscovery::new(vec![])));
        let error = resolve_java_on(21, &data, &resolver, java_path::Platform::Termux, |_, _| {
            panic!("must not download")
        })
        .await
        .unwrap_err();
        assert!(error.client_message().contains("pkg install openjdk-21"));
        assert!(!data.exists());
    }

    #[tokio::test]
    async fn installed_android_java_is_reused_without_provisioning() {
        use crate::java::CountingDiscovery;
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let java = JavaInstallation {
            home: tmp.path().join("jdk"),
            java: tmp.path().join("jdk/bin/java"),
            javac: None,
            version: java_path::JavaVersion::parse("21.0.1").unwrap(),
            vendor: None,
            architecture: java_path::Architecture::current(),
            platform: java_path::Platform::Termux,
            kind: java_path::JavaKind::Jdk,
            source: java_path::DiscoverySource::Termux,
        };
        let resolver = JavaResolver::with_backend(
            tmp.path(),
            Arc::new(CountingDiscovery::new(vec![java.clone()])),
        );
        let found = resolve_java_with(21, tmp.path(), &resolver, |_, _| {
            panic!("must not download")
        })
        .await
        .unwrap();
        assert_eq!(found.java, java.java);
    }

    #[test]
    fn setting_a_property_replaces_only_that_line() {
        let original = "#Minecraft server properties\nserver-port=25565\nmotd=Hello\n";
        let updated = set_property(original, "server-port", "25599");

        assert!(updated.contains("server-port=25599"));
        assert!(!updated.contains("25565"));
        // Everything the operator wrote has to survive.
        assert!(updated.contains("motd=Hello"));
        assert!(updated.contains("#Minecraft server properties"));
    }

    #[test]
    fn a_missing_property_is_appended() {
        let updated = set_property("motd=Hello\n", "server-port", "25565");

        assert!(updated.contains("motd=Hello"));
        assert!(updated.contains("server-port=25565"));
    }

    #[test]
    fn a_commented_out_property_is_not_mistaken_for_the_real_one() {
        let original = "#server-port=1234\nserver-port=25565\n";
        let updated = set_property(original, "server-port", "25599");

        assert!(
            updated.contains("#server-port=1234"),
            "the comment must survive"
        );
        assert!(updated.contains("server-port=25599"));
        assert!(!updated.contains("=25565"));
    }

    #[test]
    fn whitespace_around_a_key_is_tolerated() {
        let updated = set_property("server-port = 25565\n", "server-port", "25599");
        assert!(updated.contains("server-port=25599"));
        assert!(!updated.contains("25565"));
    }

    #[tokio::test]
    async fn scaffold_seeds_then_updates_the_port() {
        let tmp = tempfile::tempdir().unwrap();
        let mut config = ServerConfig::paper(tmp.path(), "1.21.8");
        config.eula_accepted = true;
        config.port = 25565;

        scaffold(&config).await.unwrap();
        let props = tmp.path().join("server.properties");
        assert!(std::fs::read_to_string(&props)
            .unwrap()
            .contains("server-port=25565"));

        // An operator edits the file by hand...
        std::fs::write(
            &props,
            "server-port=25565\nmotd=My Server\ndifficulty=hard\n",
        )
        .unwrap();

        // ...then changes the port in the panel.
        config.port = 25599;
        scaffold(&config).await.unwrap();

        let text = std::fs::read_to_string(&props).unwrap();
        assert!(
            text.contains("server-port=25599"),
            "the panel's port must win"
        );
        assert!(text.contains("motd=My Server"), "hand edits must survive");
        assert!(text.contains("difficulty=hard"));
    }

    #[test]
    fn absolute_paths_are_left_alone() {
        // Built from the cwd rather than written as a literal: "/srv/mc" is not
        // absolute on Windows, which has no drive letter to anchor it.
        let path = std::env::current_dir().unwrap().join("server.jar");
        assert!(path.is_absolute());

        assert_eq!(absolute(path.clone()), path);
    }

    #[test]
    fn relative_paths_are_resolved_against_the_cwd() {
        let resolved = absolute(PathBuf::from("./data/servers/abc/server.jar"));

        assert!(resolved.is_absolute());
        assert!(resolved.ends_with("data/servers/abc/server.jar"));
    }

    #[test]
    fn absolutizing_an_environment_fixes_every_path() {
        // A relative jar is the specific failure this guards: the JVM is spawned
        // with its cwd set to `directory`, so "./data/x/server.jar" would be
        // looked for at "<directory>/data/x/server.jar" and never found.
        let environment = ServerEnvironment {
            runtime: ServerRuntime::Java {
                java: PathBuf::from("bin/java"),
                java_major: 21,
                jar: PathBuf::from("./data/servers/abc/server.jar"),
            },
            directory: PathBuf::from("./data/servers/abc"),
        }
        .absolutize();

        let ServerRuntime::Java { java, jar, .. } = environment.runtime else {
            panic!("wrong runtime")
        };
        assert!(java.is_absolute());
        assert!(jar.is_absolute());
        assert!(environment.directory.is_absolute());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn horizon_preparation_preserves_inputs_on_failure() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let installation = root.path().join("horizon");
        let directory = root.path().join("server");
        std::fs::create_dir(&installation).unwrap();
        std::fs::create_dir(&directory).unwrap();
        for name in ["horizon-server", "horizon-engine"] {
            let path = installation.join(name);
            std::fs::write(&path, b"binary").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut config = ServerConfig::paper(&directory, "26.3");
        config.core = "horizon".into();
        assert!(matches!(
            prepare_horizon(
                &config,
                root.path(),
                crate::sandbox::SandboxPolicy::new(true),
                |_, _| {}
            )
            .await,
            Err(Error::EulaNotAccepted)
        ));
        config.eula_accepted = true;
        let original = "level-name=kept-world\nmotd=Original\n";
        std::fs::write(directory.join("server.properties"), original).unwrap();
        assert!(prepare_horizon(
            &config,
            root.path(),
            crate::sandbox::SandboxPolicy::new(true),
            |_, _| {}
        )
        .await
        .is_err());
        assert_eq!(
            std::fs::read_to_string(directory.join("server.properties")).unwrap(),
            original
        );
        assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".horizon-init-")));
        assert!(!directory.join("eula.txt").exists());
        let invalid = "[server\nport=25565\n";
        std::fs::write(directory.join("server.toml"), invalid).unwrap();
        assert!(matches!(
            prepare_horizon(
                &config,
                root.path(),
                crate::sandbox::SandboxPolicy::new(true),
                |_, _| {}
            )
            .await,
            Err(Error::InvalidConfiguration(_))
        ));
        assert_eq!(
            std::fs::read_to_string(directory.join("server.toml")).unwrap(),
            invalid
        );
        assert!(!directory.join("eula.txt").exists());
        std::fs::remove_file(directory.join("server.toml")).unwrap();
        std::fs::remove_file(directory.join("server.properties")).unwrap();
        let outside = root.path().join("outside-properties");
        std::fs::write(&outside, "do not import").unwrap();
        std::os::unix::fs::symlink(&outside, directory.join("server.properties")).unwrap();
        assert!(prepare_horizon(
            &config,
            root.path(),
            crate::sandbox::SandboxPolicy::new(true),
            |_, _| {}
        )
        .await
        .is_err());
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "do not import");
        assert!(!directory.join("server.toml").exists());
        let config_text = "[world]\nname = 'custom'\n";
        std::fs::write(directory.join("server.toml"), config_text).unwrap();
        std::fs::write(
            directory.join(".mcpanel-migration-in-progress"),
            b"incomplete",
        )
        .unwrap();
        let error = prepare_horizon(
            &config,
            root.path(),
            crate::sandbox::SandboxPolicy::new(true),
            |_, _| {},
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("migration is incomplete"));
        assert!(!directory.join("eula.txt").exists());
        std::fs::remove_file(directory.join(".mcpanel-migration-in-progress")).unwrap();
        std::fs::write(
            directory.join(".mcpanel-migrated-world.json"),
            r#"{"version":1,"world":"other"}"#,
        )
        .unwrap();
        let error = prepare_horizon(
            &config,
            root.path(),
            crate::sandbox::SandboxPolicy::new(true),
            |_, _| {},
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("does not match"));
        assert_eq!(
            std::fs::read_to_string(directory.join("server.toml")).unwrap(),
            config_text
        );
        assert!(!directory.join("eula.txt").exists());
        std::fs::remove_file(directory.join(".mcpanel-migrated-world.json")).unwrap();
        std::fs::create_dir_all(directory.join("custom_the_end/DIM1")).unwrap();
        let error = prepare_horizon(
            &config,
            root.path(),
            crate::sandbox::SandboxPolicy::new(true),
            |_, _| {},
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("custom_the_end/DIM1"));
        assert_eq!(
            std::fs::read_to_string(directory.join("server.toml")).unwrap(),
            config_text
        );
        assert!(!directory.join("eula.txt").exists());
    }

    #[test]
    fn download_stage_does_not_embed_a_percentage() {
        let stage = download_stage("paper", "26.2");

        assert_eq!(stage, "downloading paper 26.2");
        assert!(!stage.contains('%'));
    }

    #[test]
    fn paper_world_layout_requires_explicit_migration_without_modifying_data() {
        let root = tempfile::tempdir().unwrap();
        let fs = ScopedFs::open(root.path()).unwrap();
        let config = "[world]\nname = 'custom'\n"
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert!(check_paper_world_layout(&fs, root.path(), &config).is_ok());
        let legacy = root.path().join("custom_nether/DIM-1/region");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("r.0.0.mca"), b"original").unwrap();
        let error = check_paper_world_layout(&fs, root.path(), &config)
            .unwrap_err()
            .to_string();
        assert!(error.contains("custom_nether/DIM-1"));
        assert_eq!(
            std::fs::read(legacy.join("r.0.0.mca")).unwrap(),
            b"original"
        );
        // Existing vanilla dimensions may have already been regenerated; they
        // must not make the split Paper dimension silently disappear.
        std::fs::create_dir_all(root.path().join("custom/DIM-1")).unwrap();
        assert!(check_paper_world_layout(&fs, root.path(), &config).is_err());
        std::fs::rename(
            root.path().join("custom_nether"),
            root.path().join("preserved-nether"),
        )
        .unwrap();
        let paper_data = root
            .path()
            .join("custom/dimensions/minecraft/overworld/data/minecraft");
        std::fs::create_dir_all(&paper_data).unwrap();
        std::fs::write(paper_data.join("weather.dat"), b"weather").unwrap();
        let error = check_paper_world_layout(&fs, root.path(), &config)
            .unwrap_err()
            .to_string();
        assert!(error.contains("weather.dat"));
        assert_eq!(
            std::fs::read(paper_data.join("weather.dat")).unwrap(),
            b"weather"
        );
    }

    #[test]
    fn migrated_world_must_match_configuration_and_still_exist() {
        let root = tempfile::tempdir().unwrap();
        let fs = ScopedFs::open(root.path()).unwrap();
        let config = "[world]\nname = 'adventure'\n"
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert!(check_migrated_world(&fs, root.path(), &config).is_ok());
        std::fs::write(
            root.path().join(".mcpanel-migrated-world.json"),
            r#"{"version":1,"world":"other"}"#,
        )
        .unwrap();
        assert!(check_migrated_world(&fs, root.path(), &config)
            .unwrap_err()
            .to_string()
            .contains("does not match"));
        std::fs::write(
            root.path().join(".mcpanel-migrated-world.json"),
            r#"{"version":1,"world":"adventure"}"#,
        )
        .unwrap();
        assert!(check_migrated_world(&fs, root.path(), &config)
            .unwrap_err()
            .to_string()
            .contains("level.dat is missing"));
        std::fs::create_dir(root.path().join("adventure")).unwrap();
        std::fs::write(root.path().join("adventure/level.dat"), b"fixture").unwrap();
        assert!(check_migrated_world(&fs, root.path(), &config).is_ok());
        std::fs::write(root.path().join(".mcpanel-migrated-world.json"), "invalid").unwrap();
        assert!(check_migrated_world(&fs, root.path(), &config).is_err());
    }

    /// A fake but well-formed JDK home: `bin/java[.exe]` plus a `release`
    /// file. Never executed — trust validation only reads the release file.
    fn fake_jdk_home(parent: &Path, name: &str, major: u32) -> PathBuf {
        let home = parent.join(name);
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::write(home.join("bin").join(super::java_exe_name()), b"java").unwrap();
        std::fs::write(
            home.join("release"),
            format!("JAVA_VERSION=\"{major}.0.3\"\nIMPLEMENTOR=\"Test\"\n"),
        )
        .unwrap();
        std::fs::canonicalize(&home).unwrap_or(home)
    }

    fn warm_config(server_dir: &Path) -> ServerConfig {
        let mut config = ServerConfig::paper(server_dir, "1.21.8");
        config.core = "test-core".into();
        config.java_major = 21;
        config.build = Some("112".into());
        config.eula_accepted = true;
        config
    }

    fn warm_installation(server_dir: &Path, home: &Path) -> Installation {
        std::fs::write(server_dir.join("server.jar"), b"jar").unwrap();
        Installation {
            core: "test-core".into(),
            version: "1.21.8".into(),
            build: "112".into(),
            java_major: 21,
            java: home.join("bin").join(super::java_exe_name()),
            java_home: home.to_path_buf(),
            jar: server_dir.join("server.jar"),
            installed_at: "2026-08-19T00:00:00Z".into(),
        }
    }

    /// Warm Start against a system Java must reuse the installation with zero
    /// discovery passes, zero downloads and zero subprocess probes.
    ///
    /// This test fails on the old implementation, which called full
    /// `java_path::discover()` (registry queries on Windows) from
    /// `trusted_installation()`.
    #[tokio::test]
    async fn warm_start_reuses_system_java_without_any_discovery() {
        use crate::java::CountingDiscovery;
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let home = fake_jdk_home(tmp.path(), "system-jdk-21", 21);
        let server_dir = tmp.path().join("server");
        std::fs::create_dir_all(&server_dir).unwrap();
        let config = warm_config(&server_dir);
        warm_installation(&server_dir, &home)
            .save(&server_dir)
            .await
            .unwrap();

        let backend = Arc::new(CountingDiscovery::new(Vec::new()));
        let resolver = JavaResolver::with_backend(tmp.path().join("data"), backend.clone());

        let env = prepare_with_resolver(
            &config,
            tmp.path().join("data").as_path(),
            Provision::IfNeeded,
            &resolver,
            |_, _| {},
        )
        .await
        .unwrap();

        assert_eq!(backend.calls(), 0, "warm Start must not discover");
        assert_eq!(resolver.discovery_passes(), 0);
        let ServerRuntime::Java { java, jar, .. } = env.runtime else {
            panic!("wrong runtime")
        };
        assert_eq!(java, home.join("bin").join(super::java_exe_name()));
        assert_eq!(jar, server_dir.join("server.jar"));
    }

    #[test]
    fn a_managed_jdk_is_trusted_without_discovery() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let home = fake_jdk_home(&data_dir.join("jdks"), "temurin-21", 21);
        let server_dir = tmp.path().join("server");
        std::fs::create_dir_all(&server_dir).unwrap();
        let config = warm_config(&server_dir);
        let installation = warm_installation(&server_dir, &home);

        assert!(trusted_installation(&installation, &config, &data_dir));
    }

    #[test]
    fn a_record_without_a_java_home_falls_back_to_the_launcher_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let home = fake_jdk_home(tmp.path(), "legacy-jdk-21", 21);
        let server_dir = tmp.path().join("server");
        std::fs::create_dir_all(&server_dir).unwrap();
        let config = warm_config(&server_dir);
        let mut installation = warm_installation(&server_dir, &home);
        installation.java_home = PathBuf::new();

        assert!(trusted_installation(&installation, &config, tmp.path()));
    }

    #[test]
    fn an_edited_record_pointing_at_an_arbitrary_executable_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let home = fake_jdk_home(tmp.path(), "real-jdk-21", 21);
        let server_dir = tmp.path().join("server");
        std::fs::create_dir_all(&server_dir).unwrap();
        let config = warm_config(&server_dir);
        let mut installation = warm_installation(&server_dir, &home);

        let evil = tmp.path().join("malicious.exe");
        std::fs::write(&evil, b"evil").unwrap();
        installation.java = evil;

        assert!(
            !trusted_installation(&installation, &config, tmp.path()),
            "an arbitrary executable must never be trusted"
        );
    }

    #[test]
    fn a_launcher_inside_the_server_directory_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let server_dir = tmp.path().join("server");
        let home = fake_jdk_home(&server_dir, "planted-jdk", 21);
        std::fs::create_dir_all(&server_dir).unwrap();
        let config = warm_config(&server_dir);
        let installation = warm_installation(&server_dir, &home);

        assert!(
            !trusted_installation(&installation, &config, tmp.path()),
            "a launcher inside the operator-writable server directory is attacker-placed"
        );
    }

    #[test]
    fn a_home_whose_release_file_reports_another_major_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let home = fake_jdk_home(tmp.path(), "jdk-17", 17);
        let server_dir = tmp.path().join("server");
        std::fs::create_dir_all(&server_dir).unwrap();
        let config = warm_config(&server_dir);
        let installation = warm_installation(&server_dir, &home);

        assert!(!trusted_installation(&installation, &config, tmp.path()));
    }

    #[test]
    fn installer_artifacts_are_rejected_before_they_can_become_server_jar() {
        use minecraft_core::ArtifactKind;

        let error = ensure_binary_artifact("forge", ArtifactKind::Installer).unwrap_err();
        assert!(
            matches!(error, Error::UnsupportedInstaller { .. }),
            "Forge must fail loudly, not boot as server.jar"
        );
        assert!(error.client_message().contains("not supported"));

        assert!(ensure_binary_artifact("paper", ArtifactKind::Binary).is_ok());
    }
}
