/// Parses a bin spec string into `(program, prefix_args)`.
///
/// Splits on whitespace so that values like `bash /path/to/wrapper.sh`
/// are handled correctly: the first token becomes the executable and the
/// rest become arguments prepended before any caller-supplied arguments.
/// Leading `~/` in each token is expanded to the user's home directory.
fn parse_bin_spec(s: String) -> (String, Vec<String>) {
    let home = std::env::var("HOME").unwrap_or_default();
    let expand = |t: &str| -> String {
        if t.starts_with("~/") {
            format!("{}/{}", home, &t[2..])
        } else {
            t.to_string()
        }
    };
    let mut parts = s.split_whitespace().map(expand);
    let prog = parts.next().unwrap_or_default();
    (prog, parts.collect())
}

/// Returns `(executable, prefix_args)` for the `claude` agent.
///
/// Reads `HAPIR_CLAUDE_PATH` from the environment; falls back to `"claude"`.
/// The value may be a command string such as `bash /path/to/wrapper.sh`.
pub fn get_claude_bin() -> (String, Vec<String>) {
    parse_bin_spec(std::env::var("HAPIR_CLAUDE_PATH").unwrap_or_else(|_| "claude".to_string()))
}

/// Returns `(executable, prefix_args)` for the `codex` agent.
///
/// Reads `HAPIR_CODEX_PATH` from the environment; falls back to `"codex"`.
/// The value may be a command string such as `bash /path/to/wrapper.sh`.
pub fn get_codex_bin() -> (String, Vec<String>) {
    parse_bin_spec(std::env::var("HAPIR_CODEX_PATH").unwrap_or_else(|_| "codex".to_string()))
}

/// Returns `(executable, prefix_args)` for the `gemini` agent.
///
/// Reads `HAPIR_GEMINI_PATH` from the environment; falls back to `"gemini"`.
/// The value may be a command string such as `bash /path/to/wrapper.sh`.
pub fn get_gemini_bin() -> (String, Vec<String>) {
    parse_bin_spec(std::env::var("HAPIR_GEMINI_PATH").unwrap_or_else(|_| "gemini".to_string()))
}

/// Returns `(executable, prefix_args)` for the `opencode` agent.
///
/// Reads `HAPIR_OPENCODE_PATH` from the environment; falls back to `"opencode"`.
/// The value may be a command string such as `bash /path/to/wrapper.sh`.
pub fn get_opencode_bin() -> (String, Vec<String>) {
    parse_bin_spec(
        std::env::var("HAPIR_OPENCODE_PATH").unwrap_or_else(|_| "opencode".to_string()),
    )
}
