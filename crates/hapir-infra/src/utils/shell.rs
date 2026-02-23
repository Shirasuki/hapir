/// Returns the default shell for the current platform.
///
/// Unix: `$SHELL` → `/bin/zsh` (macOS) → `/bin/bash`
/// Windows: `$COMSPEC` → `powershell.exe`
pub fn default_shell() -> String {
    #[cfg(unix)]
    {
        if let Ok(shell) = std::env::var("SHELL")
            && !shell.is_empty()
        {
            return shell;
        }
        if cfg!(target_os = "macos") {
            "/bin/zsh".to_string()
        } else {
            "/bin/bash".to_string()
        }
    }
    #[cfg(windows)]
    {
        if let Ok(shell) = std::env::var("COMSPEC")
            && !shell.is_empty()
        {
            return shell;
        }
        "powershell.exe".to_string()
    }
    #[cfg(not(any(unix, windows)))]
    {
        "/bin/sh".to_string()
    }
}

/// Returns the flag used to pass a command string to the shell.
///
/// Unix shells use `-c`, `cmd.exe` uses `/C`, PowerShell uses `-Command`.
pub fn shell_command_flag(shell: &str) -> &'static str {
    let base = shell.rsplit(['/', '\\']).next().unwrap_or(shell);
    if base.eq_ignore_ascii_case("cmd.exe") || base.eq_ignore_ascii_case("cmd") {
        "/C"
    } else if base.to_ascii_lowercase().contains("powershell")
        || base.to_ascii_lowercase().contains("pwsh")
    {
        "-Command"
    } else {
        "-c"
    }
}