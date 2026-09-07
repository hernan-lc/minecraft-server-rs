//! Centralized hidden child-process construction for the tray.
//!
//! The tray's browser launch (`rundll32.exe`) must stay invisible on Windows
//! GUI sessions. Helpers created here always carry `CREATE_NO_WINDOW`, even
//! when `MCPANEL_CONSOLE=1` gives the panel itself a console.

use std::ffi::OsStr;

/// Windows `CREATE_NO_WINDOW` (`0x0800_0000`).
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Apply `CREATE_NO_WINDOW` to a std child command. No-op on other OSes.
pub fn hide(command: &mut std::process::Command) -> &mut std::process::Command {
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
pub fn hidden_command(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    hide(&mut command);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_matches_the_windows_sdk_value() {
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
    }

    #[test]
    fn hidden_command_targets_the_requested_program() {
        let command = hidden_command("rundll32.exe");
        assert_eq!(command.get_program(), OsStr::new("rundll32.exe"));
    }

    #[test]
    fn hide_is_idempotent() {
        let mut command = std::process::Command::new("rundll32.exe");
        hide(&mut command);
        hide(&mut command);
        assert_eq!(command.get_program(), OsStr::new("rundll32.exe"));
    }
}
