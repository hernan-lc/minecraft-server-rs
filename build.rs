use std::path::PathBuf;

/// Guarantee the embedded-asset directory exists before the derive runs.
///
/// `web/dist` is a build artifact and is not committed, but `#[derive(RustEmbed)]`
/// fails at compile time if its folder is missing. Without this, a fresh clone
/// cannot `cargo build` at all — it dies on a confusing macro error long before
/// reaching the "frontend not built" page that handles exactly this case.
fn main() {
    // Windows release builds are GUI executables (no console window). Setting
    // MCPANEL_CONSOLE=1 keeps the console, for headless servers or reading
    // logs from a terminal. A build-script flag rather than a Cargo feature
    // so common invocations like `--all-features` cannot silently re-add the
    // console window.
    if std::env::var_os("MCPANEL_CONSOLE").is_some_and(|value| !value.is_empty() && value != "0") {
        println!("cargo:rustc-cfg=mcpanel_console");
    }
    println!("cargo:rerun-if-env-changed=MCPANEL_CONSOLE");
    println!("cargo::rustc-check-cfg=cfg(mcpanel_console)");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets this"));
    let dist = manifest.join("web/dist");
    // Release builds may bake the target's complete Horizon runtime into the
    // panel. Ordinary source builds use an empty directory and bootstrap the
    // pinned checkout on first start instead.
    println!("cargo:rerun-if-env-changed=MCPANEL_HORIZON_EMBED");
    let horizon = if let Some(configured) = std::env::var_os("MCPANEL_HORIZON_EMBED") {
        let path = PathBuf::from(configured);
        assert!(
            path.is_dir(),
            "configured Horizon runtime directory is missing"
        );
        let suffix = if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            ".exe"
        } else {
            ""
        };
        for name in [
            format!("horizon-server{suffix}"),
            format!("horizon-engine{suffix}"),
            format!("prepare/horizon-prepare{suffix}"),
            "prepare/mods/horizon/src/test/java/dev/horizonlab/parity/PrepareWorker.java".into(),
        ] {
            let file = path.join(&name);
            assert!(
                std::fs::metadata(&file).is_ok_and(|m| m.is_file() && m.len() > 0),
                "missing embedded Horizon file: {}",
                file.display()
            );
        }
        let lock = std::fs::read(path.join("prepare/research-lock.json"))
            .expect("embedded Horizon research-lock.json is missing");
        assert_eq!(
            lock,
            std::fs::read(manifest.join("vendor/horizon/research-lock.json"))
                .expect("pinned Horizon research-lock.json is missing"),
            "embedded Horizon does not match the pinned source"
        );
        path
    } else {
        let path = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets this"))
            .join("horizon-empty");
        std::fs::create_dir_all(&path).expect("create empty Horizon embedded-asset directory");
        path
    };
    println!(
        "cargo:rustc-env=MCPANEL_HORIZON_EMBED={}",
        horizon.display()
    );
    println!("cargo:rerun-if-changed={}", horizon.display());
    watch(&horizon);

    if let Err(e) = std::fs::create_dir_all(&dist) {
        // Not fatal on its own: the derive below produces the real error if the
        // directory genuinely cannot be used.
        println!("cargo:warning=could not create {}: {e}", dist.display());
    }

    // Every embedded file is declared individually. Watching only the directory
    // is not enough: `rust-embed` bakes the files in at compile time, and cargo
    // will happily skip recompiling when a nested asset changed but the top
    // directory's own timestamp did not — leaving a stale UI inside a binary
    // that was just rebuilt.
    println!("cargo:rerun-if-changed={}", dist.display());
    watch(&dist);
}

/// Declare every file under `dir` as a build input.
fn watch(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        println!("cargo:rerun-if-changed={}", path.display());

        if path.is_dir() {
            watch(&path);
        }
    }
}
