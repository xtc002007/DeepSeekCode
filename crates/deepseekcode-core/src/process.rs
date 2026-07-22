//! Cross-platform subprocess helpers (Windows: suppress console windows).

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Configure a std `Command` so it never allocates a visible console on Windows.
pub fn hide_console(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// Configure a tokio `Command` so it never allocates a visible console on Windows.
pub fn hide_console_async(cmd: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn hide_console_sets_creation_flags() {
        let mut cmd = std::process::Command::new("cmd");
        hide_console(&mut cmd);
        // Creation flags are opaque; smoke-test only.
    }
}
