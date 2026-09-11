//! Centralized hidden child-process construction.
//!
//! Every internal helper child (the Minecraft JVM, and any future probe) must
//! stay invisible on Windows GUI sessions. `CREATE_NO_WINDOW` is applied here
//! so individual spawn sites cannot forget it. `MCPANEL_CONSOLE=1` only
//! affects the panel's own console (via `windows_subsystem`); helpers created
//! through this module always stay hidden.

use std::ffi::OsStr;

/// Windows `CREATE_NO_WINDOW` (`0x0800_0000`).
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Apply `CREATE_NO_WINDOW` to a Tokio child command. No-op on other OSes.
pub fn hide_tokio(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
    command
}

/// Build a Tokio command that stays invisible on Windows.
pub fn hidden_tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    hide_tokio(&mut command);
    command
}

/// Apply `CREATE_NO_WINDOW` to a std child command. No-op on other OSes.
pub fn hide_std(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
    command
}

/// Build a std command that stays invisible on Windows.
pub fn hidden_std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    hide_std(&mut command);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use std::process::Stdio;
    #[cfg(windows)]
    use tokio::io::AsyncReadExt;

    #[test]
    fn flag_matches_the_windows_sdk_value() {
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
    }

    #[test]
    fn hidden_commands_target_the_requested_program() {
        let tokio_cmd = hidden_tokio_command("java");
        assert_eq!(tokio_cmd.as_std().get_program(), OsStr::new("java"));
        let std_cmd = hidden_std_command("reg");
        assert_eq!(std_cmd.get_program(), OsStr::new("reg"));
    }

    #[test]
    fn hide_helpers_are_idempotent() {
        let mut tokio_cmd = tokio::process::Command::new("java");
        hide_tokio(&mut tokio_cmd);
        hide_tokio(&mut tokio_cmd);
        assert_eq!(tokio_cmd.as_std().get_program(), OsStr::new("java"));

        let mut std_cmd = std::process::Command::new("java");
        hide_std(&mut std_cmd);
        hide_std(&mut std_cmd);
        assert_eq!(std_cmd.get_program(), OsStr::new("java"));
    }

    /// Keep this Windows-only because the behavior under test is the
    /// CREATE_NO_WINDOW + piped stdio combination used for the JVM.
    #[cfg(windows)]
    #[tokio::test]
    async fn create_no_window_preserves_piped_stdout_and_stderr() {
        let command = std::env::var_os("ComSpec").expect("Windows has ComSpec");
        let mut child = hidden_tokio_command(command)
            .args(["/C", "echo stdout-pump & echo stderr-pump 1>&2"])
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("hidden child should spawn");
        let mut stdout = child.stdout.take().expect("stdout pipe");
        let mut stderr = child.stderr.take().expect("stderr pipe");
        let (stdout, stderr) = tokio::join!(
            async {
                let mut bytes = Vec::new();
                stdout.read_to_end(&mut bytes).await.unwrap();
                bytes
            },
            async {
                let mut bytes = Vec::new();
                stderr.read_to_end(&mut bytes).await.unwrap();
                bytes
            },
        );
        let status = child.wait().await.unwrap();
        assert!(status.success());
        assert!(String::from_utf8_lossy(&stdout).contains("stdout-pump"));
        assert!(String::from_utf8_lossy(&stderr).contains("stderr-pump"));
    }
}
