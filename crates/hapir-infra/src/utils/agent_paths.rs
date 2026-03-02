/// Returns the path to the `claude` executable.
///
/// Reads `HAPIR_CLAUDE_PATH` from the environment; falls back to `"claude"`.
pub fn get_claude_bin() -> String {
    std::env::var("HAPIR_CLAUDE_PATH").unwrap_or_else(|_| "claude".to_string())
}

/// Returns the path to the `codex` executable.
///
/// Reads `HAPIR_CODEX_PATH` from the environment; falls back to `"codex"`.
pub fn get_codex_bin() -> String {
    std::env::var("HAPIR_CODEX_PATH").unwrap_or_else(|_| "codex".to_string())
}

/// Returns the path to the `gemini` executable.
///
/// Reads `HAPIR_GEMINI_PATH` from the environment; falls back to `"gemini"`.
pub fn get_gemini_bin() -> String {
    std::env::var("HAPIR_GEMINI_PATH").unwrap_or_else(|_| "gemini".to_string())
}

/// Returns the path to the `opencode` executable.
///
/// Reads `HAPIR_OPENCODE_PATH` from the environment; falls back to `"opencode"`.
pub fn get_opencode_bin() -> String {
    std::env::var("HAPIR_OPENCODE_PATH").unwrap_or_else(|_| "opencode".to_string())
}
