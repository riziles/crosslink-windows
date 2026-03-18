// Types, structs, enums, and constants for the kickoff command.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Container runtime for agent execution.
#[derive(Debug, Clone, PartialEq)]
pub enum ContainerMode {
    /// Run as a local process (tmux session with claude CLI).
    None,
    /// Run inside a Docker container.
    Docker,
    /// Run inside a Podman container.
    Podman,
}

/// Post-implementation verification level.
#[derive(Debug, Clone, PartialEq)]
pub enum VerifyLevel {
    /// Local tests and self-review checklist only.
    Local,
    /// Push branch, open draft PR, wait for CI.
    Ci,
    /// CI plus structured adversarial self-review.
    Thorough,
}

/// A single acceptance criterion extracted from a design document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Criterion {
    pub id: String,
    pub text: String,
    #[serde(rename = "type")]
    pub criterion_type: String,
}

/// Machine-readable acceptance criteria file (`.kickoff-criteria.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CriteriaFile {
    pub source_doc: String,
    pub extracted_at: String,
    pub criteria: Vec<Criterion>,
}

/// Metadata written at agent launch (`.kickoff-metadata.json`).
///
/// Records the timeout and start time so that `status` / `list` can detect
/// agents that have exceeded their time budget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KickoffMetadata {
    /// ISO-8601 UTC timestamp of when the agent was launched.
    pub started_at: String,
    /// Timeout in seconds (matches `--timeout` flag).
    pub timeout_secs: u64,
}

/// Options for `crosslink kickoff run`.
pub struct KickoffOpts<'a> {
    pub description: &'a str,
    pub issue: Option<i64>,
    pub container: ContainerMode,
    pub verify: VerifyLevel,
    pub model: &'a str,
    pub image: &'a str,
    pub timeout: Duration,
    pub dry_run: bool,
    pub branch: Option<&'a str>,
    pub quiet: bool,
    pub design_doc: Option<&'a super::super::design_doc::DesignDoc>,
    pub doc_path: Option<&'a str>,
    pub skip_permissions: bool,
}

/// Parse a container mode string into the enum.
pub fn parse_container_mode(s: &str) -> Result<ContainerMode> {
    match s.to_lowercase().as_str() {
        "none" | "local" => Ok(ContainerMode::None),
        "docker" => Ok(ContainerMode::Docker),
        "podman" => Ok(ContainerMode::Podman),
        _ => bail!(
            "Unknown container runtime '{}'. Use: none, docker, podman",
            s
        ),
    }
}

/// Parse a verification level string into the enum.
pub fn parse_verify_level(s: &str) -> Result<VerifyLevel> {
    match s.to_lowercase().as_str() {
        "local" => Ok(VerifyLevel::Local),
        "ci" => Ok(VerifyLevel::Ci),
        "thorough" => Ok(VerifyLevel::Thorough),
        _ => bail!(
            "Unknown verification level '{}'. Use: local, ci, thorough",
            s
        ),
    }
}

/// A single criterion verdict in the validation report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CriterionVerdict {
    pub id: String,
    pub verdict: String,
    pub evidence: String,
}

/// Summary counts in the validation report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReportSummary {
    pub total: usize,
    pub pass: usize,
    pub fail: usize,
    pub partial: usize,
    pub not_applicable: usize,
    pub needs_clarification: usize,
}

/// Timing and metrics for a single phase of agent work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PhaseTiming {
    pub duration_s: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_read: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_modified: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines_added: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines_removed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests_run: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests_passed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests_failed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments_added: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria_checked: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issues_found: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issues_fixed: Option<u64>,
}

/// Phase-level timing breakdown for a kickoff run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PhaseTimings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exploration: Option<PhaseTiming>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning: Option<PhaseTiming>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implementation: Option<PhaseTiming>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub testing: Option<PhaseTiming>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<PhaseTiming>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<PhaseTiming>,
}

/// The `.kickoff-report.json` file contents.
///
/// Phase 3 fields (`validated_at`, `criteria`, `summary`) are always required.
/// Phase 4 fields are optional with serde defaults for backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KickoffReport {
    // Phase 3 fields (backward compat — always present)
    pub validated_at: String,
    pub criteria: Vec<CriterionVerdict>,
    pub summary: ReportSummary,

    // Phase 4 fields (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phases: Option<PhaseTimings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unresolved_questions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commits: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_changed: Option<Vec<String>>,
}

/// Check a kickoff report for missing recommended fields.
pub(crate) fn validate_kickoff_report(report: &KickoffReport) -> Vec<String> {
    let mut warnings = Vec::new();
    if report.schema_version.is_none() {
        warnings.push("Missing schema_version field".to_string());
    }
    if report.agent_id.is_none() {
        warnings.push("Missing agent_id field".to_string());
    }
    if report.issue_id.is_none() {
        warnings.push("Missing issue_id field".to_string());
    }
    if report.criteria.is_empty() {
        warnings.push("No criteria results in report".to_string());
    }
    warnings
}

/// Output format for the kickoff report command.
#[derive(Debug, Clone, PartialEq)]
pub enum ReportFormat {
    /// Human-readable table with symbols.
    Table,
    /// Raw JSON output.
    Json,
    /// PR-ready markdown format.
    Markdown,
}

/// Options for `crosslink kickoff plan`.
pub struct PlanOpts<'a> {
    pub doc: &'a super::super::design_doc::DesignDoc,
    pub model: &'a str,
    pub timeout: Duration,
    pub dry_run: bool,
    pub issue: Option<i64>,
    pub quiet: bool,
}

/// Detect project conventions from the repo root.
pub(crate) struct ProjectConventions {
    pub(crate) test_command: Option<String>,
    pub(crate) lint_commands: Vec<String>,
    pub(crate) allowed_tools: Vec<String>,
}

pub(crate) fn detect_conventions(repo_root: &Path) -> ProjectConventions {
    let mut conv = ProjectConventions {
        test_command: None,
        lint_commands: Vec::new(),
        allowed_tools: Vec::new(),
    };

    // Rust
    if repo_root.join("Cargo.toml").is_file() || repo_root.join("crosslink/Cargo.toml").is_file() {
        conv.test_command = Some("cargo test".to_string());
        conv.lint_commands
            .push("cargo clippy -- -D warnings".to_string());
        conv.lint_commands.push("cargo fmt --check".to_string());
        conv.allowed_tools.push("Bash(cargo *)".to_string());
    }

    // Node/TypeScript
    if repo_root.join("package.json").is_file() {
        if conv.test_command.is_none() {
            conv.test_command = Some("npm test".to_string());
        }
        conv.allowed_tools.push("Bash(npm *)".to_string());
        conv.allowed_tools.push("Bash(npx *)".to_string());
    }

    // Python
    if repo_root.join("pyproject.toml").is_file() || repo_root.join("requirements.txt").is_file() {
        if conv.test_command.is_none() {
            conv.test_command = Some("uv run pytest".to_string());
        }
        conv.lint_commands.push("ruff check .".to_string());
        conv.allowed_tools.push("Bash(uv *)".to_string());
        conv.allowed_tools.push("Bash(python3 *)".to_string());
    }

    // Go
    if repo_root.join("go.mod").is_file() {
        if conv.test_command.is_none() {
            conv.test_command = Some("go test ./...".to_string());
        }
        conv.lint_commands.push("go vet ./...".to_string());
        conv.allowed_tools.push("Bash(go *)".to_string());
    }

    // Just
    if repo_root.join("justfile").is_file() || repo_root.join("Justfile").is_file() {
        conv.allowed_tools.push("Bash(just *)".to_string());
    }

    // Make
    if repo_root.join("Makefile").is_file() || repo_root.join("makefile").is_file() {
        conv.allowed_tools.push("Bash(make *)".to_string());
    }

    // Elixir
    if repo_root.join("mix.exs").is_file() {
        if conv.test_command.is_none() {
            conv.test_command = Some("mix test".to_string());
        }
        conv.lint_commands
            .push("mix format --check-formatted".to_string());
        conv.allowed_tools.push("Bash(mix compile *)".to_string());
        conv.allowed_tools.push("Bash(mix test *)".to_string());
        conv.allowed_tools.push("Bash(mix format *)".to_string());
        conv.allowed_tools.push("Bash(mix deps.get *)".to_string());
        conv.allowed_tools.push("Bash(mix deps.tree *)".to_string());
        conv.allowed_tools
            .push("Bash(mix deps.compile *)".to_string());
        conv.allowed_tools
            .push("Bash(mix ecto.migrate *)".to_string());
        conv.allowed_tools
            .push("Bash(mix gettext.extract *)".to_string());
        conv.allowed_tools
            .push("Bash(mix gettext.merge *)".to_string());
        conv.allowed_tools.push("Bash(mix help *)".to_string());
        conv.allowed_tools.push("Bash(mix hex.info *)".to_string());
        conv.allowed_tools.push("Bash(mix xref *)".to_string());
        conv.allowed_tools
            .push("Bash(mix phx.routes *)".to_string());
        conv.allowed_tools.push("Bash(mix dialyzer *)".to_string());

        // Credo (check if it's a dep)
        if let Ok(content) = std::fs::read_to_string(repo_root.join("mix.exs")) {
            if content.contains(":credo") {
                conv.lint_commands.push("mix credo --strict".to_string());
                conv.allowed_tools.push("Bash(mix credo *)".to_string());
            }
            if content.contains(":sobelow") {
                conv.lint_commands.push("mix sobelow --config".to_string());
                conv.allowed_tools.push("Bash(mix sobelow *)".to_string());
            }
            // Tidewave MCP tools (if :tidewave is a dep and a local dev server is running)
            // NOTE: subagent support for starting mix phx.server is TBD — for now
            // these tools are available but require a running dev server
            if content.contains(":tidewave") {
                conv.allowed_tools
                    .push("mcp__tidewave__get_logs".to_string());
                conv.allowed_tools
                    .push("mcp__tidewave__get_source_location".to_string());
                conv.allowed_tools
                    .push("mcp__tidewave__get_docs".to_string());
                conv.allowed_tools
                    .push("mcp__tidewave__get_ecto_schemas".to_string());
                conv.allowed_tools
                    .push("mcp__tidewave__search_package_docs".to_string());
                conv.allowed_tools
                    .push("mcp__tidewave__list_project_files".to_string());
                conv.allowed_tools
                    .push("mcp__tidewave__read_project_file".to_string());
                conv.allowed_tools
                    .push("mcp__tidewave__grep_project_files".to_string());
                conv.allowed_tools
                    .push("mcp__tidewave__execute_sql_query".to_string());
                conv.allowed_tools
                    .push("mcp__tidewave__project_eval".to_string());
            }
        }
    }

    conv
}

/// Compute which patterns need adding to a git exclude file.
///
/// Given the existing exclude file content, returns only the patterns
/// from `KICKOFF_EXCLUDE_PATTERNS` that are not already present.
pub(crate) const KICKOFF_EXCLUDE_PATTERNS: &[&str] = &[
    "KICKOFF.md",
    ".kickoff-status",
    ".kickoff-slug",
    ".kickoff-metadata.json",
    "PLAN_KICKOFF.md",
    ".kickoff-plan.json",
    ".kickoff-criteria.json",
    ".kickoff-report.json",
];

pub(crate) fn missing_exclude_patterns(existing_content: &str) -> Vec<&'static str> {
    KICKOFF_EXCLUDE_PATTERNS
        .iter()
        .filter(|pattern| !existing_content.lines().any(|l| l.trim() == **pattern))
        .copied()
        .collect()
}

/// Information about a discovered kickoff agent.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AgentInfo {
    pub(crate) id: String,
    pub(crate) issue: Option<String>,
    pub(crate) status: String,
    pub(crate) session: Option<String>,
    pub(crate) worktree: String,
    pub(crate) docker: Option<String>,
}

/// Classification of an agent for cleanup purposes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) enum CleanupClass {
    /// Agent confirmed done — safe to remove.
    Done,
    /// Agent appears stale (no tmux/container, no DONE sentinel).
    Stale,
    /// Agent is still active — do not touch.
    Active,
}

/// Result of a single agent cleanup action.
#[derive(Debug, Serialize)]
pub(crate) struct CleanupResult {
    pub(crate) id: String,
    pub(crate) class: CleanupClass,
    pub(crate) worktree_removed: bool,
    pub(crate) tmux_killed: bool,
    pub(crate) container_removed: bool,
    pub(crate) error: Option<String>,
}

/// Detected platform for generating targeted install instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Platform {
    MacOS,
    Linux(LinuxDistro),
    Windows,
}

/// Known Linux distribution families.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinuxDistro {
    Debian,
    Fedora,
    Arch,
    Alpine,
    Other,
}

/// Result of a successful pre-flight check.
pub(crate) struct PreflightResult {
    /// The resolved timeout command (`timeout` or `gtimeout`).
    pub(crate) timeout_cmd: &'static str,
    /// Optional sandbox wrapper command from hook-config.json `sandbox.command`.
    pub(crate) sandbox_command: Option<String>,
}

/// Watchdog configuration for detecting and nudging idle agents.
pub(crate) struct WatchdogConfig {
    /// Whether the watchdog is enabled (default: true)
    pub(crate) enabled: bool,
    /// Seconds of heartbeat staleness before nudging (default: 300)
    pub(crate) staleness_secs: u64,
    /// Maximum number of nudges before giving up (default: 5)
    pub(crate) max_nudges: u32,
    /// Seconds between watchdog checks (default: 120)
    pub(crate) check_interval_secs: u64,
    /// Grace period before watchdog starts checking (default: 300)
    pub(crate) grace_period_secs: u64,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            staleness_secs: 300,
            max_nudges: 5,
            check_interval_secs: 120,
            grace_period_secs: 300,
        }
    }
}
