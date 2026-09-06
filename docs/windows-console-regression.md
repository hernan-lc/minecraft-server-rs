# Windows console-window regression checklist

Run this checklist against a packaged Windows release built with
`MCPANEL_CONSOLE` unset. A development `cargo run` console is expected and is
not the release configuration under test.

- [ ] Launch MCP Panel from Explorer.
- [ ] Create and start a server with a system Java installation.
- [ ] Trigger Java discovery, including registry discovery.
- [ ] Trigger the fallback Java metadata inspection if possible.
- [ ] If no matching Java exists, install/download Java.
- [ ] Start Minecraft and confirm its output is captured by the panel console.
- [ ] Restart Minecraft.
- [ ] Open the browser from the tray icon.
- [ ] Confirm no `cmd.exe`, `reg.exe`, `java.exe`, PowerShell, or extra
      `rundll32.exe` console window flashes during these operations.

The release binary must retain the following behavior when
`MCPANEL_CONSOLE=1` is set: the panel itself may have a development console,
but internal helper and captured child processes must still remain hidden.
