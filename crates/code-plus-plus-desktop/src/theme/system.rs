//! OS color-scheme detection for `ThemeId::System`.

/// Returns `true` when the OS prefers a dark appearance.
pub fn prefers_dark() -> bool {
    #[cfg(windows)]
    {
        use std::process::Command;
        let mut cmd = Command::new("reg");
        code_plus_plus_core::process::hide_console(&mut cmd);
        let output = cmd
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                "/v",
                "AppsUseLightTheme",
            ])
            .output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            // 0x0 = dark, 0x1 = light
            if stdout.contains("0x0") {
                return true;
            }
            if stdout.contains("0x1") {
                return false;
            }
        }
        true
    }
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let output = Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            return stdout.trim() == "Dark";
        }
        true
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
            if content.contains("ubuntu") || content.contains("debian") {
                use std::process::Command;
                let output = Command::new("gsettings")
                    .args(["get", "org.gnome.desktop.interface", "color-scheme"])
                    .output();
                if let Ok(out) = output {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    return stdout.contains("dark");
                }
            }
        }
        true
    }
    #[cfg(not(any(windows, target_os = "macos", unix)))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_dark_is_bool() {
        let _ = prefers_dark();
    }
}
