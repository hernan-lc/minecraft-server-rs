//! Install only the Horizon programs shipped beside the running panel.

use crate::error::{ApiError, ApiResult};
use guardian::ScopedFs;
use std::path::{Path, PathBuf};

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
