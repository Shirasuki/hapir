use anyhow::{Result, bail};
use hapir_infra::utils::agent_paths::get_claude_bin;
use std::io::ErrorKind;
use std::process::Command;
use tracing::debug;

const MIN_CLAUDE_VERSION: (u32, u32, u32) = (2, 1, 47);

fn parse_version(output: &str) -> Option<(u32, u32, u32)> {
    let token = output.split_whitespace().next()?;
    let mut parts = token.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

pub fn check_claude_version() -> Result<()> {
    // Skip version check when HAPIR_CLAUDE_PATH is explicitly set to a custom value.
    // A custom wrapper may not support --version correctly.
    if std::env::var("HAPIR_CLAUDE_PATH").is_ok() {
        debug!("HAPIR_CLAUDE_PATH is set, skipping Claude version check");
        return Ok(());
    }

    let (claude_prog, claude_prefix) = get_claude_bin();
    let output = match Command::new(claude_prog).args(&claude_prefix).arg("--version").output() {
        Ok(o) => o,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            bail!("Claude Code 未安装。请先安装: npm install -g @anthropic-ai/claude-code");
        }
        Err(e) => bail!("无法执行 claude --version: {e}"),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let version_output = if stdout.trim().is_empty() { &stderr } else { &stdout };
    let version = parse_version(version_output)
        .ok_or_else(|| anyhow::anyhow!("无法解析 Claude Code 版本号，输出: {version_output}"))?;

    if version < MIN_CLAUDE_VERSION {
        bail!(
            "Claude Code 版本过低 ({}.{}.{})，需要 >= {}.{}.{}。请升级: npm update -g @anthropic-ai/claude-code",
            version.0,
            version.1,
            version.2,
            MIN_CLAUDE_VERSION.0,
            MIN_CLAUDE_VERSION.1,
            MIN_CLAUDE_VERSION.2,
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_normal() {
        assert_eq!(parse_version("2.1.50 (Claude Code)"), Some((2, 1, 50)));
    }

    #[test]
    fn test_parse_version_bare() {
        assert_eq!(parse_version("2.1.47"), Some((2, 1, 47)));
    }

    #[test]
    fn test_parse_version_empty() {
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn test_parse_version_garbage() {
        assert_eq!(parse_version("not-a-version"), None);
    }

    #[test]
    fn test_parse_version_incomplete() {
        assert_eq!(parse_version("2.1"), None);
    }

    #[test]
    fn test_version_comparison() {
        assert!((2, 1, 47) >= MIN_CLAUDE_VERSION);
        assert!((2, 1, 48) >= MIN_CLAUDE_VERSION);
        assert!((2, 2, 0) >= MIN_CLAUDE_VERSION);
        assert!((3, 0, 0) >= MIN_CLAUDE_VERSION);
        assert!((2, 1, 46) < MIN_CLAUDE_VERSION);
        assert!((2, 0, 99) < MIN_CLAUDE_VERSION);
        assert!((1, 9, 99) < MIN_CLAUDE_VERSION);
    }
}
