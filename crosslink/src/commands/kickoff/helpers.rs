// Helper utility functions used across kickoff submodules (and by swarm).

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use super::types::{
    KickoffMetadata, LinuxDistro, Platform, PreflightResult, WatchdogConfig,
};

/// Parse a human-readable duration string (e.g. "1h", "30m", "90s") into Duration.
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        bail!("Empty duration string");
    }

    let (num_str, unit) = if let Some(n) = s.strip_suffix('h') {
        (n, 'h')
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 'm')
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 's')
    } else {
        // Bare number defaults to seconds
        (s, 's')
    };

    let value: u64 = num_str
        .parse()
        .with_context(|| format!("Invalid duration number: '{}'", num_str))?;

    let secs = match unit {
        'h' => value * 3600,
        'm' => value * 60,
        's' => value,
        _ => unreachable!(),
    };

    if secs == 0 {
        bail!("Duration must be greater than zero");
    }

    Ok(Duration::from_secs(secs))
}

/// Slugify a feature description into a branch-safe name.
pub(crate) fn slugify(description: &str) -> String {
    let slug: String = description
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();

    // Collapse multiple hyphens and trim
    let mut result = String::new();
    let mut prev_hyphen = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_hyphen && !result.is_empty() {
                result.push('-');
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }

    // Trim trailing hyphens and truncate
    let trimmed = result.trim_end_matches('-');
    if trimmed.len() > 60 {
        // Cut at the last hyphen before 60 chars to avoid mid-word
        match trimmed[..60].rfind('-') {
            Some(pos) => trimmed[..pos].to_string(),
            None => trimmed[..60].to_string(),
        }
    } else {
        trimmed.to_string()
    }
}

/// Format seconds as a human-readable duration string.
pub(crate) fn format_duration(secs: u64) -> String {
    if secs >= 3600 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m > 0 {
            format!("{}h {}m", h, m)
        } else {
            format!("{}h", h)
        }
    } else if secs >= 60 {
        let m = secs / 60;
        let s = secs % 60;
        if s > 0 {
            format!("{}m {}s", m, s)
        } else {
            format!("{}m", m)
        }
    } else {
        format!("{}s", secs)
    }
}

/// Derive a tmux session name from the branch slug.
pub(crate) fn tmux_session_name(slug: &str) -> String {
    let name = format!("feat-{}", slug);
    let sanitized: String = name
        .chars()
        .map(|c| if c == '.' || c == ':' { '-' } else { c })
        .collect();
    if sanitized.len() > 50 {
        sanitized[..50].to_string()
    } else {
        sanitized
    }
}

/// Check if a tmux session with the given name already exists.
pub(crate) fn tmux_session_exists(name: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if a command is available on PATH.
pub(crate) fn command_available(cmd: &str) -> bool {
    #[cfg(target_os = "windows")]
    let lookup = Command::new("where.exe").arg(cmd).output();
    #[cfg(not(target_os = "windows"))]
    let lookup = Command::new("which").arg(cmd).output();

    lookup.map(|o| o.status.success()).unwrap_or(false)
}

/// Detect the current platform and Linux distribution (if applicable).
pub(crate) fn detect_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::MacOS
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Linux(detect_linux_distro())
    }
}

/// Detect the Linux distribution by reading /etc/os-release.
fn detect_linux_distro() -> LinuxDistro {
    let content = match std::fs::read_to_string("/etc/os-release") {
        Ok(c) => c.to_lowercase(),
        Err(_) => return LinuxDistro::Other,
    };
    if content.contains("id=debian")
        || content.contains("id=ubuntu")
        || content.contains("id_like=debian")
        || content.contains("id_like=\"debian")
    {
        LinuxDistro::Debian
    } else if content.contains("id=fedora")
        || content.contains("id=rhel")
        || content.contains("id=centos")
        || content.contains("id_like=fedora")
        || content.contains("id_like=\"fedora")
        || content.contains("id_like=\"rhel")
    {
        LinuxDistro::Fedora
    } else if content.contains("id=arch")
        || content.contains("id_like=arch")
        || content.contains("id_like=\"arch")
    {
        LinuxDistro::Arch
    } else if content.contains("id=alpine") {
        LinuxDistro::Alpine
    } else {
        LinuxDistro::Other
    }
}

/// Build a platform-specific install hint for a given command.
pub(crate) fn install_hint(cmd: &str, platform: &Platform) -> String {
    match cmd {
        "timeout" | "gtimeout" => match platform {
            Platform::MacOS => "On macOS, install GNU coreutils:\n\
                 \n  brew install coreutils\n\
                 \nThis provides `gtimeout` which crosslink will use automatically."
                .to_string(),
            Platform::Linux(LinuxDistro::Debian) => {
                "Install coreutils (provides `timeout`):\n\n  sudo apt install coreutils"
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Fedora) => {
                "Install coreutils (provides `timeout`):\n\n  sudo dnf install coreutils"
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Arch) => {
                "Install coreutils (provides `timeout`):\n\n  sudo pacman -S coreutils".to_string()
            }
            Platform::Linux(LinuxDistro::Alpine) => {
                "Install coreutils (provides `timeout`):\n\n  apk add coreutils".to_string()
            }
            Platform::Linux(LinuxDistro::Other) => {
                "Install GNU coreutils to get the `timeout` command.\n\
                 Use your distribution's package manager (e.g. apt, dnf, pacman)."
                    .to_string()
            }
            Platform::Windows => "Install GNU coreutils via scoop or chocolatey:\n\
                 \n  scoop install coreutils\n  choco install gnuwin32-coreutils.install"
                .to_string(),
        },
        "tmux" => match platform {
            Platform::MacOS => "`tmux` is not installed.\n\n  brew install tmux\n\
                 \nAlternatively, use --container docker to avoid tmux."
                .to_string(),
            Platform::Linux(LinuxDistro::Debian) => {
                "`tmux` is not installed.\n\n  sudo apt install tmux\n\
                 \nAlternatively, use --container docker to avoid tmux."
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Fedora) => {
                "`tmux` is not installed.\n\n  sudo dnf install tmux\n\
                 \nAlternatively, use --container docker to avoid tmux."
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Arch) => {
                "`tmux` is not installed.\n\n  sudo pacman -S tmux\n\
                 \nAlternatively, use --container docker to avoid tmux."
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Alpine) => {
                "`tmux` is not installed.\n\n  apk add tmux\n\
                 \nAlternatively, use --container docker to avoid tmux."
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Other) => "`tmux` is not installed.\n\
                 Install with your distribution's package manager (e.g. apt, dnf, pacman).\n\
                 \nAlternatively, use --container docker to avoid tmux."
                .to_string(),
            Platform::Windows => "`tmux` is not available on Windows.\n\
                 Use --container docker instead for containerized agent mode."
                .to_string(),
        },
        "claude" => match platform {
            Platform::MacOS => "`claude` CLI is not installed.\n\n  brew install claude-code\n\
                 \nOr install via npm:\n\n  npm install -g @anthropic-ai/claude-code"
                .to_string(),
            Platform::Windows => {
                "`claude` CLI is not installed.\n\n  npm install -g @anthropic-ai/claude-code"
                    .to_string()
            }
            Platform::Linux(_) => {
                "`claude` CLI is not installed.\n\n  npm install -g @anthropic-ai/claude-code"
                    .to_string()
            }
        },
        "gh" => match platform {
            Platform::MacOS => {
                "`gh` (GitHub CLI) is required for --verify ci/thorough.\n\n  brew install gh"
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Debian) => {
                "`gh` (GitHub CLI) is required for --verify ci/thorough.\n\
                 \nInstall via apt (official repo):\n\
                 \n  sudo mkdir -p /etc/apt/keyrings\n  \
                 curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
                 | sudo tee /etc/apt/keyrings/githubcli-archive-keyring.gpg > /dev/null\n  \
                 echo \"deb [arch=$(dpkg --print-architecture) \
                 signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] \
                 https://cli.github.com/packages stable main\" \
                 | sudo tee /etc/apt/sources.list.d/github-cli.list > /dev/null\n  \
                 sudo apt update && sudo apt install gh\n\
                 \nOr install a single binary from: https://cli.github.com"
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Fedora) => {
                "`gh` (GitHub CLI) is required for --verify ci/thorough.\
                 \n\n  sudo dnf install gh"
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Arch) => {
                "`gh` (GitHub CLI) is required for --verify ci/thorough.\
                 \n\n  sudo pacman -S github-cli"
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Alpine) => {
                "`gh` (GitHub CLI) is required for --verify ci/thorough.\
                 \n\n  apk add github-cli"
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Other) => {
                "`gh` (GitHub CLI) is required for --verify ci/thorough.\n\
                 Install from: https://cli.github.com"
                    .to_string()
            }
            Platform::Windows => "`gh` (GitHub CLI) is required for --verify ci/thorough.\
                 \n\n  winget install GitHub.cli\n\
                 \nOr: scoop install gh"
                .to_string(),
        },
        "docker" => match platform {
            Platform::MacOS => "`docker` is not installed.\n\n  brew install --cask docker\n\
                 \nOr install Docker Desktop from: https://docs.docker.com/get-docker/\n\
                 \nAlternatively, use --container none for local mode."
                .to_string(),
            Platform::Linux(LinuxDistro::Debian) => "`docker` is not installed.\n\
                 \nInstall Docker Engine:\n\
                 \n  curl -fsSL https://get.docker.com | sh\n  sudo usermod -aG docker $USER\n\
                 \nOr see: https://docs.docker.com/engine/install/\n\
                 \nAlternatively, use --container none for local mode."
                .to_string(),
            Platform::Linux(LinuxDistro::Fedora) => "`docker` is not installed.\n\
                 \n  sudo dnf install docker-ce docker-ce-cli containerd.io\n\
                 \nOr: curl -fsSL https://get.docker.com | sh\n\
                 \nAlternatively, use --container none for local mode."
                .to_string(),
            Platform::Linux(LinuxDistro::Arch) => "`docker` is not installed.\n\
                 \n  sudo pacman -S docker\n  sudo systemctl enable --now docker\n\
                 \nAlternatively, use --container none for local mode."
                .to_string(),
            Platform::Linux(LinuxDistro::Alpine) => "`docker` is not installed.\n\
                 \n  apk add docker\n  rc-update add docker default\n  service docker start\n\
                 \nAlternatively, use --container none for local mode."
                .to_string(),
            Platform::Linux(LinuxDistro::Other) | Platform::Windows => {
                "`docker` is not installed.\n\
                 Install from: https://docs.docker.com/get-docker/\n\
                 \nAlternatively, use --container none for local mode."
                    .to_string()
            }
        },
        "podman" => match platform {
            Platform::MacOS => "`podman` is not installed.\n\n  brew install podman\n\
                 \nAlternatively, use --container none for local mode."
                .to_string(),
            Platform::Linux(LinuxDistro::Debian) => {
                "`podman` is not installed.\n\n  sudo apt install podman\n\
                 \nAlternatively, use --container none for local mode."
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Fedora) => {
                "`podman` is not installed.\n\n  sudo dnf install podman\n\
                 \nAlternatively, use --container none for local mode."
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Arch) => {
                "`podman` is not installed.\n\n  sudo pacman -S podman\n\
                 \nAlternatively, use --container none for local mode."
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Alpine) => {
                "`podman` is not installed.\n\n  apk add podman\n\
                 \nAlternatively, use --container none for local mode."
                    .to_string()
            }
            Platform::Linux(LinuxDistro::Other) => "`podman` is not installed.\n\
                 Install from: https://podman.io/getting-started/installation\n\
                 \nAlternatively, use --container none for local mode."
                .to_string(),
            Platform::Windows => "`podman` is not installed.\n\
                 \n  winget install RedHat.Podman\n\
                 \nAlternatively, use --container none for local mode."
                .to_string(),
        },
        other => format!(
            "`{}` is not installed. Install it using your system package manager.",
            other
        ),
    }
}

/// Resolve the correct `timeout` command for the current platform.
///
/// On macOS, `timeout` is not available by default. The GNU coreutils
/// package (via Homebrew) installs it as `gtimeout`.
/// Returns the command name to use, or an error with install instructions.
pub(crate) fn resolve_timeout_command(platform: &Platform) -> Result<&'static str> {
    if command_available("timeout") {
        return Ok("timeout");
    }
    if command_available("gtimeout") {
        return Ok("gtimeout");
    }
    bail!(
        "Neither `timeout` nor `gtimeout` found.\n{}",
        install_hint("timeout", platform)
    );
}

/// Read the `sandbox.command` setting from hook-config.json, if configured.
pub(crate) fn read_sandbox_command(crosslink_dir: &Path) -> Option<String> {
    let config_path = crosslink_dir.join("hook-config.json");
    let content = std::fs::read_to_string(&config_path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
    parsed
        .get("sandbox")
        .and_then(|s| s.get("command"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

pub(crate) fn read_watchdog_config(crosslink_dir: &Path) -> WatchdogConfig {
    let config_path = crosslink_dir.join("hook-config.json");
    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return WatchdogConfig::default(),
    };
    let parsed: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return WatchdogConfig::default(),
    };

    let wd = match parsed.get("watchdog") {
        Some(v) => v,
        None => return WatchdogConfig::default(),
    };

    let mut cfg = WatchdogConfig::default();
    if let Some(v) = wd.get("enabled").and_then(|v| v.as_bool()) {
        cfg.enabled = v;
    }
    if let Some(v) = wd.get("staleness_secs").and_then(|v| v.as_u64()) {
        cfg.staleness_secs = v;
    }
    if let Some(v) = wd.get("max_nudges").and_then(|v| v.as_u64()) {
        cfg.max_nudges = v as u32;
    }
    if let Some(v) = wd.get("check_interval_secs").and_then(|v| v.as_u64()) {
        cfg.check_interval_secs = v;
    }
    if let Some(v) = wd.get("grace_period_secs").and_then(|v| v.as_u64()) {
        cfg.grace_period_secs = v;
    }
    cfg
}

/// Pre-flight check: verify all required external commands are present before
/// creating worktrees, branches, or sessions. Emits clear errors with install
/// instructions for any missing command.
pub(crate) fn preflight_check(
    container: &super::types::ContainerMode,
    verify: &super::types::VerifyLevel,
    crosslink_dir: &Path,
) -> Result<PreflightResult> {
    let platform = detect_platform();
    let mut missing: Vec<String> = Vec::new();

    // timeout (or gtimeout on macOS) — always required for agent timeout
    let timeout_cmd = match resolve_timeout_command(&platform) {
        Ok(cmd) => cmd,
        Err(e) => {
            missing.push(format!("{}", e));
            "timeout" // placeholder, won't be used since we'll bail
        }
    };

    // tmux — required for local (non-container) mode
    // On Windows, tmux is not available at all — bail early with a clear message.
    if *container == super::types::ContainerMode::None {
        if cfg!(target_os = "windows") {
            bail!(
                "Local kickoff mode requires tmux, which is not available on Windows.\n\
                 Use `--container docker` for agent kickoff on Windows."
            );
        }
        if !command_available("tmux") {
            missing.push(install_hint("tmux", &platform));
        }
    }

    // claude CLI — required for local mode
    if *container == super::types::ContainerMode::None && !command_available("claude") {
        missing.push(install_hint("claude", &platform));
    }

    // gh — required for CI/thorough verification
    if (*verify == super::types::VerifyLevel::Ci
        || *verify == super::types::VerifyLevel::Thorough)
        && !command_available("gh")
    {
        missing.push(install_hint("gh", &platform));
    }

    // docker/podman — required when using container mode
    match container {
        super::types::ContainerMode::Docker if !command_available("docker") => {
            missing.push(install_hint("docker", &platform));
        }
        super::types::ContainerMode::Podman if !command_available("podman") => {
            missing.push(install_hint("podman", &platform));
        }
        _ => {}
    }

    // sandbox command — validate the binary exists when configured
    let sandbox_command = read_sandbox_command(crosslink_dir);
    if let Some(ref cmd) = sandbox_command {
        // Extract the binary name (first word before any flags/templates)
        let binary = cmd.split_whitespace().next().unwrap_or(cmd);
        if !command_available(binary) {
            missing.push(format!(
                "`{}` (configured in hook-config.json sandbox.command) not found on PATH",
                binary
            ));
        }
    }

    if !missing.is_empty() {
        let header = format!(
            "Pre-flight check failed — {} missing command{}:\n",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" }
        );
        let body = missing
            .iter()
            .enumerate()
            .map(|(i, msg)| format!("{}. {}", i + 1, msg))
            .collect::<Vec<_>>()
            .join("\n\n");
        bail!("{}{}", header, body);
    }

    Ok(PreflightResult {
        timeout_cmd,
        sandbox_command,
    })
}

/// Get the git repository root.
pub(crate) fn repo_root() -> Result<std::path::PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("Failed to run git rev-parse")?;
    if !output.status.success() {
        bail!("Not inside a git repository");
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(std::path::PathBuf::from(path))
}

/// Generate a small random numeric suffix (no external crate needed).
pub(crate) fn rand_suffix() -> u32 {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    seed % 10000
}

/// Generate a 4-character hex suffix for worktree directory uniqueness.
///
/// Combines nanosecond timestamp with process ID to avoid collisions
/// when two processes start in the same nanosecond window.
pub(crate) fn rand_hex_suffix() -> String {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let pid = std::process::id();
    let mixed = nanos.wrapping_mul(31).wrapping_add(pid);
    format!("{:04x}", mixed & 0xFFFF)
}

/// Format the verification level as a display string.
pub(crate) fn verify_level_name(level: &super::types::VerifyLevel) -> &'static str {
    match level {
        super::types::VerifyLevel::Local => "local",
        super::types::VerifyLevel::Ci => "ci",
        super::types::VerifyLevel::Thorough => "thorough",
    }
}

/// Normalize raw status file content to a canonical status string.
pub(crate) fn normalize_status(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower == "done" {
        "done".to_string()
    } else if lower.contains("fail") || lower.contains("error") {
        "failed".to_string()
    } else if lower.contains("running") || raw.is_empty() {
        "running".to_string()
    } else {
        raw.to_string()
    }
}

/// Check if an agent has exceeded its timeout based on `.kickoff-metadata.json`.
///
/// Returns `true` if the metadata file exists, contains a valid start time and
/// timeout, and the elapsed wall-clock time exceeds the configured timeout.
pub(crate) fn is_timed_out(wt_path: &Path) -> bool {
    let meta_path = wt_path.join(".kickoff-metadata.json");
    let content = match std::fs::read_to_string(&meta_path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let meta: KickoffMetadata = match serde_json::from_str(&content) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let started = match chrono::DateTime::parse_from_rfc3339(&meta.started_at) {
        Ok(dt) => dt.with_timezone(&chrono::Utc),
        Err(_) => return false,
    };
    let elapsed = chrono::Utc::now().signed_duration_since(started);
    elapsed.num_seconds() > meta.timeout_secs as i64
}

/// Read the timeout metadata for display purposes.
pub(crate) fn read_timeout_metadata(wt_path: &Path) -> Option<KickoffMetadata> {
    let meta_path = wt_path.join(".kickoff-metadata.json");
    let content = std::fs::read_to_string(&meta_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Read the agent ID from the worktree's .crosslink/agent.json.
pub(crate) fn read_agent_id(wt_path: &Path, _crosslink_dir: &Path) -> Option<String> {
    let agent_json = wt_path.join(".crosslink").join("agent.json");
    if agent_json.exists() {
        if let Ok(content) = std::fs::read_to_string(&agent_json) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                return val
                    .get("agent_id")
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
        }
    }
    None
}

/// Try to read the associated issue from kickoff metadata.
pub(crate) fn read_agent_issue(wt_path: &Path, _crosslink_dir: &Path) -> Option<String> {
    // Try .kickoff-criteria.json first (has issue ID from kickoff)
    let criteria_path = wt_path.join(".kickoff-criteria.json");
    if criteria_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&criteria_path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                // The criteria file might have issue_id in extracted metadata
                if let Some(id) = val.get("issue_id").and_then(|v| v.as_i64()) {
                    return Some(format!("#{}", id));
                }
            }
        }
    }
    // Try .crosslink/agent.json
    let agent_json = wt_path.join(".crosslink").join("agent.json");
    if agent_json.exists() {
        if let Ok(content) = std::fs::read_to_string(&agent_json) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(id) = val.get("issue_id").and_then(|v| v.as_i64()) {
                    return Some(format!("#{}", id));
                }
            }
        }
    }
    None
}

/// Truncate a string to `max` characters (char-boundary safe).
pub(crate) fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        s.chars().take(max).collect()
    } else {
        s.to_string()
    }
}
