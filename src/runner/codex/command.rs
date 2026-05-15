use std::path::Path;

const CODEX_SUBCOMMANDS: &[&str] = &[
    "exec",
    "e",
    "review",
    "login",
    "logout",
    "mcp",
    "plugin",
    "mcp-server",
    "app-server",
    "app",
    "completion",
    "update",
    "sandbox",
    "debug",
    "apply",
    "a",
    "resume",
    "fork",
    "cloud",
    "exec-server",
    "features",
    "help",
];

pub(crate) fn normalize_codex_command(command: &[String]) -> Vec<String> {
    let Some(program) = command.first() else {
        return Vec::new();
    };
    if !is_codex_executable(program) || command[1..].iter().any(|arg| is_codex_subcommand(arg)) {
        return command.to_vec();
    }

    let mut normalized = command.to_vec();
    normalized.push("exec".to_string());
    normalized
}

fn is_codex_executable(program: &str) -> bool {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == "codex")
        .unwrap_or(false)
}

fn is_codex_subcommand(arg: &str) -> bool {
    CODEX_SUBCOMMANDS.contains(&arg)
}
