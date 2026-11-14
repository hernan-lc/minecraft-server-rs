use std::path::PathBuf;
use std::process::Command;

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets manifest"))
        .join("../..");
    println!(
        "cargo:rerun-if-changed={}",
        root.join(".git/HEAD").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join(".git/index").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join(".gitmodules").display()
    );
    if let Ok(reference) = Command::new("git")
        .current_dir(&root)
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
    {
        if reference.status.success() {
            let name = String::from_utf8_lossy(&reference.stdout);
            if let Ok(path) = Command::new("git")
                .current_dir(&root)
                .args(["rev-parse", "--git-path", name.trim()])
                .output()
            {
                if path.status.success() {
                    println!(
                        "cargo:rerun-if-changed={}",
                        root.join(String::from_utf8_lossy(&path.stdout).trim())
                            .display()
                    );
                }
            }
        }
    }
    if let Ok(output) = Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "HEAD:vendor/horizon"])
        .output()
    {
        if output.status.success() {
            let revision = String::from_utf8_lossy(&output.stdout);
            let revision = revision.trim();
            if revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit()) {
                println!("cargo:rustc-env=MCPANEL_HORIZON_REVISION={revision}");
            }
        }
    }
}
