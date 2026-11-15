//! Install only the Horizon programs shipped beside the running panel.

use crate::error::{ApiError, ApiResult};
use guardian::ScopedFs;
use rust_embed::RustEmbed;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(RustEmbed)]
#[folder = "$MCPANEL_HORIZON_EMBED"]
struct EmbeddedHorizon;

/// Whether this executable contains a complete Horizon runtime.
pub fn embedded_programs() -> bool {
    let suffix = std::env::consts::EXE_SUFFIX;
    [
        format!("horizon-server{suffix}"),
        format!("horizon-engine{suffix}"),
        format!("prepare/horizon-prepare{suffix}"),
        "prepare/research-lock.json".into(),
        "prepare/mods/horizon/src/test/java/dev/horizonlab/parity/PrepareWorker.java".into(),
    ]
    .iter()
    .all(|name| EmbeddedHorizon::get(name).is_some())
}

/// Install a trusted bundled runtime on first use, leaving existing installs alone.
pub fn install_if_bundled(executable: &Path, data_dir: &Path) -> ApiResult<()> {
    let installation = data_dir.join("horizon");
    if installation.exists() {
        if embedded_programs()
            && guardian::sandbox::horizon_programs_present(&installation)
            && !installation.join("prepare").exists()
        {
            install_embedded_preparer(&installation)?;
        }
        return Ok(());
    }
    if !embedded_programs() && bundled_programs(executable, data_dir).is_none() {
        return Ok(());
    }
    match install_bundled(executable, data_dir) {
        Err(ApiError::Conflict(_))
            if guardian::sandbox::horizon_programs_present(&data_dir.join("horizon")) =>
        {
            Ok(())
        }
        result => result,
    }
}

fn install_embedded_preparer(installation: &Path) -> ApiResult<()> {
    let files = EmbeddedHorizon::iter()
        .filter(|name| name.starts_with("prepare/"))
        .map(|name| {
            let value = EmbeddedHorizon::get(&name).ok_or_else(|| {
                ApiError::Internal(anyhow::anyhow!("embedded preparer file disappeared"))
            })?;
            Ok((name.into_owned(), value.data))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    install_preparer_entries(installation, files)
}

fn install_preparer_entries<'a>(
    installation: &Path,
    files: Vec<(String, std::borrow::Cow<'a, [u8]>)>,
) -> ApiResult<()> {
    // Refuse symlinked installations and parent components before creating
    // anything; an older user-managed runtime must never redirect this write.
    ScopedFs::open(installation).map_err(|e| ApiError::Internal(e.into()))?;
    let binary = format!("prepare/horizon-prepare{}", std::env::consts::EXE_SUFFIX);
    let lock = include_bytes!("../vendor/horizon/research-lock.json");
    if !files
        .iter()
        .any(|(name, bytes)| name == "prepare/research-lock.json" && bytes.as_ref() == lock)
        || !files.iter().any(|(name, _)| name == &binary)
        || !files.iter().any(|(name, _)| {
            name == "prepare/mods/horizon/src/test/java/dev/horizonlab/parity/PrepareWorker.java"
        })
        || files.len() > 20_000
    {
        return Err(ApiError::Conflict(
            "embedded Horizon preparer is incomplete".into(),
        ));
    }
    let destination = installation.join("prepare");
    std::fs::create_dir(&destination).map_err(|e| ApiError::Internal(e.into()))?;
    struct Cleanup(PathBuf, bool);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if !self.1 {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
    let mut cleanup = Cleanup(destination.clone(), false);
    let target = ScopedFs::open(&destination).map_err(|e| ApiError::Internal(e.into()))?;
    target
        .set_private()
        .map_err(|e| ApiError::Internal(e.into()))?;
    let mut total = 0u64;
    for (name, bytes) in files {
        let raw = name
            .strip_prefix("prepare/")
            .ok_or_else(|| ApiError::Conflict("invalid embedded preparer path".into()))?;
        if raw.contains(['\\', ':'])
            || raw.starts_with('/')
            || raw
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(ApiError::Conflict("invalid embedded preparer path".into()));
        }
        total = total.saturating_add(bytes.len() as u64);
        if total > 256 * 1024 * 1024 {
            return Err(ApiError::Conflict("embedded preparer is too large".into()));
        }
        let path = Path::new(raw);
        if let Some(parent) = path.parent() {
            target
                .create_dir_all(parent)
                .map_err(|e| ApiError::Internal(e.into()))?;
        }
        let mut output = target
            .create_new_file(path)
            .map_err(|e| ApiError::Internal(e.into()))?;
        output
            .write_all(&bytes)
            .map_err(|e| ApiError::Internal(e.into()))?;
        #[cfg(unix)]
        if name == binary {
            use std::os::unix::fs::PermissionsExt;
            output
                .set_permissions(std::fs::Permissions::from_mode(0o700))
                .map_err(|e| ApiError::Internal(e.into()))?;
        }
        output
            .sync_all()
            .map_err(|e| ApiError::Internal(e.into()))?;
    }
    cleanup.1 = true;
    Ok(())
}

/// Locate a release bundle without accepting a path from an API caller.
pub fn bundled_programs(executable: &Path, data_dir: &Path) -> Option<PathBuf> {
    let executable = std::fs::canonicalize(executable).ok()?;
    let parent = executable.parent()?;
    let runtime = parent.join("runtime");
    let bundle = runtime.join("horizon");
    for path in [&runtime, &bundle] {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return None;
        }
    }
    let bundle = std::fs::canonicalize(bundle).ok()?;
    let data_dir = std::fs::canonicalize(data_dir).ok()?;
    if bundle.starts_with(data_dir.join("servers")) {
        return None;
    }
    let names = [
        format!("horizon-server{}", std::env::consts::EXE_SUFFIX),
        format!("horizon-engine{}", std::env::consts::EXE_SUFFIX),
    ];
    for name in &names {
        let metadata = std::fs::symlink_metadata(bundle.join(name)).ok()?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return None;
        }
    }
    // Bundled executables are trusted like the panel executable itself. On
    // Unix require the same owner and no group/other writes in this subtree.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let owner = std::fs::metadata(&executable).ok()?.uid();
        for path in [
            parent,
            runtime.as_path(),
            bundle.as_path(),
            &bundle.join(&names[0]),
            &bundle.join(&names[1]),
        ] {
            let metadata = std::fs::metadata(path).ok()?;
            if metadata.uid() != owner || metadata.mode() & 0o022 != 0 {
                return None;
            }
        }
    }
    guardian::sandbox::horizon_programs_present(&bundle).then_some(bundle)
}

/// Copy the bundled engine first and publish the supervisor only after its
/// complete file has been written. An existing installation is never replaced.
pub fn install_bundled(executable: &Path, data_dir: &Path) -> ApiResult<()> {
    if embedded_programs() {
        return install_embedded(data_dir);
    }
    let bundle = bundled_programs(executable, data_dir).ok_or_else(|| {
        ApiError::Conflict("no trusted Horizon programs are bundled with this panel".into())
    })?;
    let source = ScopedFs::open(&bundle).map_err(|e| ApiError::Internal(e.into()))?;
    let destination = data_dir.join("horizon");
    std::fs::create_dir(&destination).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            ApiError::Conflict(
                "a Horizon installation already exists; it will not be replaced".into(),
            )
        } else {
            ApiError::Internal(error.into())
        }
    })?;
    let target = ScopedFs::open(&destination).map_err(|e| ApiError::Internal(e.into()))?;
    target
        .set_private()
        .map_err(|e| ApiError::Internal(e.into()))?;
    if bundle.join("prepare").exists() {
        copy_preparer(&bundle, &source, &target, executable)?;
    }
    for name in ["horizon-engine", "horizon-server"] {
        let name = format!("{name}{}", std::env::consts::EXE_SUFFIX);
        let temporary = format!(".{name}.installing");
        let mut input = source
            .open_file(&name)
            .map_err(|e| ApiError::Internal(e.into()))?;
        let mut output = target
            .create_new_file(&temporary)
            .map_err(|e| ApiError::Internal(e.into()))?;
        std::io::copy(&mut input, &mut output).map_err(|e| ApiError::Internal(e.into()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            output
                .set_permissions(std::fs::Permissions::from_mode(0o700))
                .map_err(|e| ApiError::Internal(e.into()))?;
        }
        output
            .sync_all()
            .map_err(|e| ApiError::Internal(e.into()))?;
        drop(output);
        target
            .rename(&temporary, &name)
            .map_err(|e| ApiError::Internal(e.into()))?;
    }
    Ok(())
}

fn install_embedded(data_dir: &Path) -> ApiResult<()> {
    let entries = EmbeddedHorizon::iter()
        .map(|name| {
            let file = EmbeddedHorizon::get(&name).ok_or_else(|| {
                ApiError::Internal(anyhow::anyhow!("embedded Horizon file disappeared"))
            })?;
            Ok((name.into_owned(), file.data))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    install_embedded_entries(data_dir, entries)
}

fn install_embedded_entries<'a>(
    data_dir: &Path,
    mut entries: Vec<(String, std::borrow::Cow<'a, [u8]>)>,
) -> ApiResult<()> {
    let suffix = std::env::consts::EXE_SUFFIX;
    let server = format!("horizon-server{suffix}");
    let engine = format!("horizon-engine{suffix}");
    let preparer = format!("prepare/horizon-prepare{suffix}");
    let expected_lock = include_bytes!("../vendor/horizon/research-lock.json");
    if !entries.iter().any(|(name, value)| {
        name == "prepare/research-lock.json" && value.as_ref() == expected_lock
    }) || ![&server, &engine, &preparer]
        .iter()
        .all(|required| entries.iter().any(|(name, _)| name == *required))
        || !entries.iter().any(|(name, _)| {
            name == "prepare/mods/horizon/src/test/java/dev/horizonlab/parity/PrepareWorker.java"
        })
    {
        return Err(ApiError::Conflict(
            "embedded Horizon runtime is incomplete or does not match this panel".into(),
        ));
    }
    // Publish the supervisor last, so an incomplete copy is never runnable.
    entries.sort_by_key(|(name, _)| name == &server);
    let destination = data_dir.join("horizon");
    std::fs::create_dir(&destination).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            ApiError::Conflict(
                "a Horizon installation already exists; it will not be replaced".into(),
            )
        } else {
            ApiError::Internal(error.into())
        }
    })?;
    struct Cleanup(PathBuf, bool);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if !self.1 {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
    let mut cleanup = Cleanup(destination.clone(), false);
    let target = ScopedFs::open(&destination).map_err(|e| ApiError::Internal(e.into()))?;
    target
        .set_private()
        .map_err(|e| ApiError::Internal(e.into()))?;
    let mut total = 0u64;
    if entries.len() > 20_000 {
        return Err(ApiError::Conflict(
            "embedded Horizon runtime has too many files".into(),
        ));
    }
    for (name, bytes) in entries {
        if name.contains(['\\', ':'])
            || name.starts_with('/')
            || name
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(ApiError::Conflict(
                "embedded Horizon runtime contains an invalid path".into(),
            ));
        }
        total = total.saturating_add(bytes.len() as u64);
        if total > 2 * 1024 * 1024 * 1024 {
            return Err(ApiError::Conflict(
                "embedded Horizon runtime is too large".into(),
            ));
        }
        let path = Path::new(&name);
        if let Some(parent) = path.parent() {
            target
                .create_dir_all(parent)
                .map_err(|e| ApiError::Internal(e.into()))?;
        }
        let mut file = target
            .create_new_file(path)
            .map_err(|e| ApiError::Internal(e.into()))?;
        file.write_all(&bytes)
            .map_err(|e| ApiError::Internal(e.into()))?;
        #[cfg(unix)]
        if name == server || name == engine || name == preparer {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o700))
                .map_err(|e| ApiError::Internal(e.into()))?;
        }
        file.sync_all().map_err(|e| ApiError::Internal(e.into()))?;
    }
    cleanup.1 = true;
    Ok(())
}

fn copy_preparer(
    bundle: &Path,
    source: &ScopedFs,
    target: &ScopedFs,
    executable: &Path,
) -> ApiResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let item = std::fs::symlink_metadata(bundle.join("prepare"))
            .map_err(|e| ApiError::Internal(e.into()))?;
        let owner = std::fs::metadata(executable)
            .map_err(|e| ApiError::Internal(e.into()))?
            .uid();
        if !item.is_dir() || item.uid() != owner || item.mode() & 0o022 != 0 {
            return Err(ApiError::Conflict(
                "untrusted Horizon preparer directory".into(),
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = (bundle, executable);
    let required = format!("prepare/horizon-prepare{}", std::env::consts::EXE_SUFFIX);
    let expected_lock = include_bytes!("../vendor/horizon/research-lock.json");
    if source
        .read_file("prepare/research-lock.json")
        .ok()
        .as_deref()
        != Some(expected_lock.as_slice())
    {
        return Err(ApiError::Conflict(
            "bundled Horizon preparer does not match this panel".into(),
        ));
    }
    let mut pending = vec![PathBuf::from("prepare")];
    let mut files = 0usize;
    let mut total = 0u64;
    while let Some(directory) = pending.pop() {
        target
            .create_dir_all(&directory)
            .map_err(|e| ApiError::Internal(e.into()))?;
        for (name, metadata) in source
            .read_dir(&directory)
            .map_err(|e| ApiError::Internal(e.into()))?
        {
            let path = directory.join(name);
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let item = std::fs::symlink_metadata(bundle.join(&path))
                    .map_err(|e| ApiError::Internal(e.into()))?;
                let owner = std::fs::metadata(executable)
                    .map_err(|e| ApiError::Internal(e.into()))?
                    .uid();
                if item.file_type().is_symlink() || item.uid() != owner || item.mode() & 0o022 != 0
                {
                    return Err(ApiError::Conflict(
                        "untrusted Horizon preparer files".into(),
                    ));
                }
            }
            if metadata.is_dir {
                pending.push(path);
            } else if metadata.is_file {
                files += 1;
                total = total.saturating_add(metadata.len);
                if files > 20_000 || total > 256 * 1024 * 1024 {
                    return Err(ApiError::Conflict("Horizon preparer is too large".into()));
                }
                let mut input = source
                    .open_file(&path)
                    .map_err(|e| ApiError::Internal(e.into()))?;
                let mut output = target
                    .create_new_file(&path)
                    .map_err(|e| ApiError::Internal(e.into()))?;
                std::io::copy(&mut input, &mut output).map_err(|e| ApiError::Internal(e.into()))?;
                #[cfg(unix)]
                if path == Path::new(&required) {
                    use std::os::unix::fs::PermissionsExt;
                    output
                        .set_permissions(std::fs::Permissions::from_mode(0o700))
                        .map_err(|e| ApiError::Internal(e.into()))?;
                }
            }
        }
    }
    if !target.metadata(&required).is_ok_and(|m| m.is_file)
        || !target
            .metadata("prepare/mods/horizon/src/test/java/dev/horizonlab/parity/PrepareWorker.java")
            .is_ok_and(|m| m.is_file)
    {
        return Err(ApiError::Conflict(
            "bundled Horizon preparer is incomplete".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &Path) -> (PathBuf, PathBuf) {
        let executable = root.join("package/mcpanel");
        let bundle = root.join("package/runtime/horizon");
        let data = root.join("data");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::create_dir_all(data.join("servers")).unwrap();
        std::fs::write(&executable, b"panel").unwrap();
        for name in ["horizon-server", "horizon-engine"] {
            std::fs::write(
                bundle.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)),
                name,
            )
            .unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in ["horizon-server", "horizon-engine"] {
                std::fs::set_permissions(bundle.join(name), std::fs::Permissions::from_mode(0o700))
                    .unwrap();
            }
        }
        (executable, data)
    }

    #[test]
    fn installs_a_bundle_without_executing_or_overwriting_programs() {
        let root = tempfile::tempdir().unwrap();
        let (exe, data) = fixture(root.path());
        assert!(bundled_programs(&exe, &data).is_some());
        install_bundled(&exe, &data).unwrap();
        let server = data.join(format!(
            "horizon/horizon-server{}",
            std::env::consts::EXE_SUFFIX
        ));
        assert_eq!(std::fs::read(&server).unwrap(), b"horizon-server");
        assert!(guardian::sandbox::horizon_programs_present(
            &data.join("horizon")
        ));
        assert!(matches!(
            install_bundled(&exe, &data),
            Err(ApiError::Conflict(_))
        ));
        assert_eq!(std::fs::read(server).unwrap(), b"horizon-server");
    }

    #[test]
    fn automatic_install_uses_the_bundle_once_and_preserves_existing_files() {
        let root = tempfile::tempdir().unwrap();
        let (exe, data) = fixture(root.path());
        install_if_bundled(&exe, &data).unwrap();
        let server = data.join(format!(
            "horizon/horizon-server{}",
            std::env::consts::EXE_SUFFIX
        ));
        std::fs::write(&server, b"existing").unwrap();
        install_if_bundled(&exe, &data).unwrap();
        assert_eq!(std::fs::read(server).unwrap(), b"existing");
    }

    #[test]
    fn installs_the_preparer_and_exporter_sources_from_a_bundle() {
        let root = tempfile::tempdir().unwrap();
        let (exe, data) = fixture(root.path());
        let prep = exe.parent().unwrap().join("runtime/horizon/prepare");
        let java = prep.join("mods/horizon/src/test/java/dev/horizonlab/parity");
        std::fs::create_dir_all(&java).unwrap();
        std::fs::write(
            prep.join("research-lock.json"),
            include_bytes!("../vendor/horizon/research-lock.json"),
        )
        .unwrap();
        std::fs::write(
            prep.join(format!("horizon-prepare{}", std::env::consts::EXE_SUFFIX)),
            b"preparer",
        )
        .unwrap();
        std::fs::write(java.join("PrepareWorker.java"), b"source").unwrap();
        install_bundled(&exe, &data).unwrap();
        assert_eq!(std::fs::read(data.join("horizon/prepare/mods/horizon/src/test/java/dev/horizonlab/parity/PrepareWorker.java")).unwrap(), b"source");
        assert!(data
            .join(format!(
                "horizon/prepare/horizon-prepare{}",
                std::env::consts::EXE_SUFFIX
            ))
            .is_file());
    }

    #[test]
    fn embedded_runtime_installs_without_a_companion_folder() {
        use std::borrow::Cow;
        let root = tempfile::tempdir().unwrap();
        let suffix = std::env::consts::EXE_SUFFIX;
        let entries = vec![
            (
                format!("horizon-server{suffix}"),
                Cow::Borrowed(b"server".as_slice()),
            ),
            (
                format!("horizon-engine{suffix}"),
                Cow::Borrowed(b"engine".as_slice()),
            ),
            (
                format!("prepare/horizon-prepare{suffix}"),
                Cow::Borrowed(b"prepare".as_slice()),
            ),
            (
                "prepare/research-lock.json".into(),
                Cow::Borrowed(include_bytes!("../vendor/horizon/research-lock.json").as_slice()),
            ),
            (
                "prepare/mods/horizon/src/test/java/dev/horizonlab/parity/PrepareWorker.java"
                    .into(),
                Cow::Borrowed(b"java".as_slice()),
            ),
        ];
        install_embedded_entries(root.path(), entries.clone()).unwrap();
        assert_eq!(
            std::fs::read(root.path().join("horizon").join(&entries[0].0)).unwrap(),
            b"server"
        );
        assert!(matches!(
            install_embedded_entries(root.path(), entries.clone()),
            Err(ApiError::Conflict(_))
        ));
        let installation = root.path().join("horizon");
        std::fs::remove_dir_all(installation.join("prepare")).unwrap();
        let preparer_entries = entries
            .into_iter()
            .filter(|(name, _)| name.starts_with("prepare/"))
            .collect();
        install_preparer_entries(&installation, preparer_entries).unwrap();
        assert_eq!(
            std::fs::read(installation.join(format!("horizon-engine{suffix}"))).unwrap(),
            b"engine"
        );
        assert!(installation
            .join(format!("prepare/horizon-prepare{suffix}"))
            .is_file());
    }

    #[test]
    fn concurrent_installs_do_not_replace_each_other() {
        let root = tempfile::tempdir().unwrap();
        let (exe, data) = fixture(root.path());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let exe = exe.clone();
                let data = data.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    install_bundled(&exe, &data)
                })
            })
            .collect();
        let outcomes: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
        assert!(guardian::sandbox::horizon_programs_present(
            &data.join("horizon")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_writable_bundles_symlinks_and_server_owned_paths() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let (exe, data) = fixture(root.path());
        let bundle = exe.parent().unwrap().join("runtime/horizon");
        let engine = bundle.join("horizon-engine");
        std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o722)).unwrap();
        assert!(bundled_programs(&exe, &data).is_none());
        std::fs::remove_file(&engine).unwrap();
        std::os::unix::fs::symlink("horizon-server", &engine).unwrap();
        assert!(bundled_programs(&exe, &data).is_none());
        let nested = data.join("servers");
        let (nested_exe, _) = fixture(&nested);
        assert!(bundled_programs(&nested_exe, &data).is_none());
    }
}
