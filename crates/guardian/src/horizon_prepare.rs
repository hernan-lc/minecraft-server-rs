//! Prepare a shared, verified Horizon resource set on first use.
use crate::{Error, JavaResolver, Result, ScopedFs};
use java_path::SelectExt;
use std::path::Path;
#[cfg(target_os = "android")]
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::io::{AsyncBufReadExt, BufReader};

static PREPARATION: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
const LOCK: &[u8] = include_bytes!("../../../vendor/horizon/research-lock.json");

/// Whether a server explicitly configured its own resource path.
pub fn has_custom_resources(directory: &Path) -> bool {
    ScopedFs::open(directory)
        .ok()
        .and_then(|fs| fs.read_file("server.toml").ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .and_then(|document| {
            document
                .get("engine")?
                .get("resources")?
                .as_str()
                .map(str::to_owned)
        })
        .is_some_and(|value| !value.trim().is_empty())
}

/// Prepare the shared game resources if neither the server nor the host has them.
/// Existing resource directories and user worlds are never replaced.
pub async fn prepare_resources(
    installation: &Path,
    directory: &Path,
    data_dir: &Path,
    resolver: &JavaResolver,
    progress: impl Fn(String, Option<f32>) + Send + Sync + Clone + 'static,
) -> Result<()> {
    let _guard = PREPARATION
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let version = crate::config::HORIZON_VERSION;
    let shared_parent = installation.join(".local/horizon-client");
    let shared = shared_parent.join(version);
    // A server-specific resource directory always takes precedence. Leave it to
    // Horizon to validate an existing directory rather than replacing it.
    if directory.join("resources").exists() || shared.exists() {
        return Ok(());
    }
    let preparer_root = installation.join("prepare");
    let preparer = preparer_root.join(format!("horizon-prepare{}", std::env::consts::EXE_SUFFIX));
    let scope = ScopedFs::open(&preparer_root).map_err(|_| {
        Error::InvalidConfiguration(
            "Horizon resource preparer is missing; install a complete Horizon runtime".into(),
        )
    })?;
    let binary = format!("horizon-prepare{}", std::env::consts::EXE_SUFFIX);
    if !scope.metadata(&binary).is_ok_and(|m| m.is_file)
        || scope.read_file("research-lock.json").ok().as_deref() != Some(LOCK)
        || !scope
            .metadata("mods/horizon/src/test/java/dev/horizonlab/parity/PrepareWorker.java")
            .is_ok_and(|m| m.is_file)
    {
        return Err(Error::InvalidConfiguration(
            "Horizon resource preparer is incomplete or does not match this panel".into(),
        ));
    }
    progress(
        "resolving Java 25 for Minecraft resource preparation".into(),
        None,
    );
    let java = resolve_java(data_dir, resolver, progress.clone()).await?;
    let inputs = data_dir.join("horizon-downloads").join(version);
    let report = progress.clone();
    crate::horizon_fetch::download_inputs(&inputs, move |label, done, total| {
        report(
            label.to_owned(),
            (total > 0).then(|| done as f32 / total as f32),
        );
    })
    .await
    .map_err(|error| {
        Error::InvalidConfiguration(format!("Minecraft download failed: {error:#}"))
    })?;
    std::fs::create_dir_all(&shared_parent).map_err(|e| Error::io(&shared_parent, e))?;
    let stage = tempfile::Builder::new()
        .prefix(".mcpanel-prepare-")
        .tempdir_in(&shared_parent)
        .map_err(|e| Error::io(&shared_parent, e))?;
    let output = stage.path().join("generated");
    progress("preparing Minecraft resources with Horizon".into(), None);
    run_preparer(
        &preparer,
        &preparer_root,
        &inputs,
        &java.home,
        &output,
        &progress,
    )
    .await?;
    // Horizon's own importer checks the manifest and the contents before the
    // resulting directory can be published for any server.
    let validation = stage.path().join("validation");
    std::fs::create_dir_all(validation.join("resources")).map_err(|e| Error::io(&validation, e))?;
    let server = installation.join(format!("horizon-server{}", std::env::consts::EXE_SUFFIX));
    let result = crate::hidden::hidden_tokio_command(&server)
        .arg("--dir")
        .arg(&validation)
        .arg("import-resources")
        .arg(&output)
        .env_clear()
        .envs(crate::process::sanitized_environment(std::env::vars_os()))
        .env("HOME", &validation)
        .env("USERPROFILE", &validation)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| Error::io(&server, e))?;
    if !result.status.success() {
        return Err(Error::InvalidConfiguration(format!(
            "Horizon rejected the generated game resources: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        )));
    }
    let validated = validation.join("resources");
    publish_resources(&validated, &shared)?;
    progress("Minecraft resources ready".into(), Some(1.0));
    Ok(())
}

fn publish_resources(validated: &Path, shared: &Path) -> Result<()> {
    let manifest = validated.join("horizon-resources.json");
    let metadata = std::fs::symlink_metadata(&manifest).map_err(|e| Error::io(&manifest, e))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Error::InvalidConfiguration(
            "validated Horizon resources have no regular manifest".into(),
        ));
    }
    // Claim a new directory rather than replacing one that appeared while the
    // download ran. Publish the manifest last so an interruption is visible.
    std::fs::create_dir(shared).map_err(|e| Error::io(shared, e))?;
    struct Cleanup<'a>(&'a Path, bool);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            if !self.1 {
                let _ = std::fs::remove_dir_all(self.0);
            }
        }
    }
    let mut cleanup = Cleanup(shared, false);
    let mut entries = std::fs::read_dir(validated)
        .map_err(|e| Error::io(validated, e))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|e| Error::io(validated, e))?;
    entries.sort_by_key(|entry| entry.file_name() == "horizon-resources.json");
    for entry in entries {
        std::fs::rename(entry.path(), shared.join(entry.file_name()))
            .map_err(|e| Error::io(shared, e))?;
    }
    cleanup.1 = true;
    Ok(())
}

async fn resolve_java(
    data_dir: &Path,
    resolver: &JavaResolver,
    progress: impl Fn(String, Option<f32>) + Send + Sync + Clone + 'static,
) -> Result<java_path::JavaInstallation> {
    if let Ok(installs) = resolver.javas() {
        if let Ok(jdk) = installs.select().major(25).current_arch().jdk().best() {
            return Ok(jdk.clone());
        }
    }
    #[cfg(target_os = "android")]
    {
        let _ = data_dir;
        progress("installing Java 25 using Termux packages".into(), None);
        let pkg = std::env::var_os("PREFIX")
            .map(PathBuf::from)
            .map(|prefix| prefix.join("bin/pkg"))
            .ok_or(Error::TermuxJavaRequired(25))?;
        install_termux_java(&pkg).await?;
        resolver.invalidate();
        let installs = resolver
            .javas()
            .map_err(|_| Error::TermuxJavaRequired(25))?;
        installs
            .select()
            .major(25)
            .current_arch()
            .jdk()
            .best()
            .cloned()
            .map_err(|_| Error::TermuxJavaRequired(25))
    }
    #[cfg(not(target_os = "android"))]
    {
        let jdk_dir = data_dir.join("jdks");
        std::fs::create_dir_all(&jdk_dir).map_err(|e| Error::io(&jdk_dir, e))?;
        progress("installing Java 25 for resource preparation".into(), None);
        java_path::JavaInstaller::adoptium()
            .version(25)
            .install_dir(&jdk_dir)
            .cache_dir(jdk_dir.join(".cache"))
            .on_event(move |event| {
                if let java_path::InstallEvent::Downloading { downloaded, total } = event {
                    progress(
                        "downloading Java 25".into(),
                        total
                            .filter(|n| *n > 0)
                            .map(|n| downloaded as f32 / n as f32),
                    );
                }
            })
            .install()
            .await
            .map_err(|error| Error::JavaUnavailable(25, error.to_string()))
    }
}

#[cfg(target_os = "android")]
async fn install_termux_java(pkg: &Path) -> Result<()> {
    let status = crate::hidden::hidden_tokio_command(pkg)
        .args(["install", "-y", "openjdk-25"])
        .env("DEBIAN_FRONTEND", "noninteractive")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|e| Error::io(pkg, e))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::TermuxJavaRequired(25))
    }
}

async fn run_preparer(
    executable: &Path,
    root: &Path,
    inputs: &Path,
    java_home: &Path,
    output: &Path,
    progress: &(impl Fn(String, Option<f32>) + Send + Sync),
) -> Result<()> {
    let mut command = crate::hidden::hidden_tokio_command(executable);
    command
        .env_clear()
        .envs(crate::process::sanitized_environment(std::env::vars_os()))
        .env("HOME", root)
        .env("USERPROFILE", root);
    #[cfg(target_os = "android")]
    command.env("HORIZON_PREPARE_JOBS", "1");
    let mut child = command
        .arg("--prepare")
        .arg("--server-jar")
        .arg(inputs.join("server.jar"))
        .arg("--minecraft-dir")
        .arg(inputs)
        .arg("--java-home")
        .arg(java_home)
        .arg("--root")
        .arg(root)
        .arg("--output")
        .arg(output)
        .arg("--progress")
        .arg("json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| Error::io(executable, e))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::InvalidConfiguration("Horizon preparer has no output".into()))?;
    let mut lines = BufReader::new(stdout).lines();
    let mut last_error = None;
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| Error::io(executable, e))?
    {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(message) = value.get("message").and_then(|v| v.as_str()) {
                progress(message.into(), None);
            }
            if let Some(fraction) = value.get("fraction").and_then(|v| v.as_f64()) {
                progress(
                    "preparing Minecraft resources".into(),
                    Some(fraction.clamp(0.0, 1.0) as f32),
                );
            }
            if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
                last_error = Some(error.to_owned());
            }
        }
    }
    let status = child.wait().await.map_err(|e| Error::io(executable, e))?;
    if !status.success() {
        return Err(Error::InvalidConfiguration(format!(
            "Horizon resource preparation failed: {}",
            last_error.unwrap_or_else(|| status.to_string())
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishing_never_replaces_existing_resources() {
        let root = tempfile::tempdir().unwrap();
        let validated = root.path().join("validated");
        let shared = root.path().join("shared");
        std::fs::create_dir(&validated).unwrap();
        std::fs::write(validated.join("horizon-resources.json"), b"manifest").unwrap();
        std::fs::write(validated.join("worldgen.json"), b"generated").unwrap();
        std::fs::create_dir(&shared).unwrap();
        std::fs::write(shared.join("user-file"), b"keep").unwrap();
        assert!(publish_resources(&validated, &shared).is_err());
        assert_eq!(std::fs::read(shared.join("user-file")).unwrap(), b"keep");
        let fresh = root.path().join("fresh");
        publish_resources(&validated, &fresh).unwrap();
        assert_eq!(
            std::fs::read(fresh.join("worldgen.json")).unwrap(),
            b"generated"
        );
        assert_eq!(
            std::fs::read(fresh.join("horizon-resources.json")).unwrap(),
            b"manifest"
        );
    }

    #[test]
    fn recognizes_an_explicit_resource_path() {
        let root = tempfile::tempdir().unwrap();
        assert!(!has_custom_resources(root.path()));
        std::fs::write(
            root.path().join("server.toml"),
            "[engine]\nresources = ''\n",
        )
        .unwrap();
        assert!(!has_custom_resources(root.path()));
        std::fs::write(
            root.path().join("server.toml"),
            "[engine]\nresources = '/custom/game-files'\n",
        )
        .unwrap();
        assert!(has_custom_resources(root.path()));
    }

    #[tokio::test]
    async fn preparation_selects_a_jdk_when_a_jre_is_also_installed() {
        use crate::java::CountingDiscovery;
        use java_path::{
            Architecture, DiscoverySource, JavaInstallation, JavaKind, JavaVersion, Platform,
        };
        let make = |home: &str, kind: JavaKind| JavaInstallation {
            home: home.into(),
            java: Path::new(home).join("bin/java"),
            javac: (kind == JavaKind::Jdk).then(|| Path::new(home).join("bin/javac")),
            version: JavaVersion::parse("25.0.1").unwrap(),
            vendor: None,
            architecture: Architecture::current(),
            platform: Platform::current(),
            kind,
            source: DiscoverySource::UserDirectory,
        };
        let backend = std::sync::Arc::new(CountingDiscovery::new(vec![
            make("/jre", JavaKind::Jre),
            make("/jdk", JavaKind::Jdk),
        ]));
        let resolver = JavaResolver::with_backend("/data", backend);
        let selected = resolve_java(Path::new("/data"), &resolver, |_, _| {})
            .await
            .unwrap();
        assert_eq!(selected.home, Path::new("/jdk"));
    }

    #[cfg(target_os = "android")]
    #[tokio::test]
    async fn termux_installs_only_the_required_jdk_package() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let pkg = root.path().join("pkg");
        std::fs::write(
            &pkg,
            b"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/args\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&pkg, std::fs::Permissions::from_mode(0o700)).unwrap();
        install_termux_java(&pkg).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("args")).unwrap(),
            "install\n-y\nopenjdk-25\n"
        );
    }

    #[tokio::test]
    async fn existing_resources_are_left_untouched_without_java_or_network() {
        let root = tempfile::tempdir().unwrap();
        let installation = root.path().join("horizon");
        let server = root.path().join("server");
        std::fs::create_dir_all(server.join("resources")).unwrap();
        std::fs::write(server.join("resources/user-file"), b"keep").unwrap();
        let resolver = JavaResolver::new(root.path());
        prepare_resources(&installation, &server, root.path(), &resolver, |_, _| {})
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(server.join("resources/user-file")).unwrap(),
            b"keep"
        );
        assert!(!installation.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preparer_progress_and_errors_reach_the_panel() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::{Arc, Mutex};
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("preparer");
        std::fs::write(
            &binary,
            b"#!/bin/sh\nprintf '%s\\n' '{\"message\":\"working\"}' '{\"fraction\":0.5}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        run_preparer(
            &binary,
            root.path(),
            root.path(),
            root.path(),
            root.path(),
            &move |message, fraction| {
                captured.lock().unwrap().push((message, fraction));
            },
        )
        .await
        .unwrap();
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|(message, _)| message == "working"));
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|(_, fraction)| *fraction == Some(0.5)));
        std::fs::write(
            &binary,
            b"#!/bin/sh\nprintf '%s\\n' '{\"error\":\"bad input\"}'\nexit 1\n",
        )
        .unwrap();
        let error = run_preparer(
            &binary,
            root.path(),
            root.path(),
            root.path(),
            root.path(),
            &|_, _| {},
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("bad input"));
    }
}
