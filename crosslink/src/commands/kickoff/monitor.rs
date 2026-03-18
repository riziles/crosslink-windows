// Status, logs, stop, and report commands for kickoff agents.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

use super::helpers::{
    command_available, format_duration, is_timed_out, read_timeout_metadata, tmux_session_exists,
    tmux_session_name, truncate_str,
};
use super::prompt::format_phase_line;
use super::types::{
    validate_kickoff_report, KickoffReport, PhaseTiming, ReportFormat,
};

/// `crosslink kickoff status <agent>`
pub fn status(crosslink_dir: &Path, agent: &str) -> Result<()> {
    // Check for .kickoff-status in any matching worktree
    let root = crosslink_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine repo root"))?;

    // Try to find the worktree by agent ID or branch slug
    let slug = agent
        .strip_prefix("feature/")
        .or_else(|| agent.strip_prefix("feat-"))
        .unwrap_or(agent);

    // Also try splitting on -- (agent IDs are parent--slug)
    let wt_slug = slug.rsplit("--").next().unwrap_or(slug);

    let worktree_dir = root.join(".worktrees").join(wt_slug);

    if !worktree_dir.exists() {
        // Try scanning all worktrees
        let worktrees_dir = root.join(".worktrees");
        if worktrees_dir.is_dir() {
            println!("Available worktrees:");
            for entry in std::fs::read_dir(&worktrees_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    let name = entry.file_name();
                    let status_file = entry.path().join(".kickoff-status");
                    let mut status = if status_file.exists() {
                        std::fs::read_to_string(&status_file)
                            .unwrap_or_default()
                            .trim()
                            .to_string()
                    } else {
                        "running".to_string()
                    };
                    if status == "running" && is_timed_out(&entry.path()) {
                        status = "timed-out".to_string();
                    }
                    println!("  {} — {}", name.to_string_lossy(), status);
                }
            }
        } else {
            println!("No worktrees found.");
        }
        return Ok(());
    }

    // Check .kickoff-status
    let status_file = worktree_dir.join(".kickoff-status");
    let mut agent_status = if status_file.exists() {
        std::fs::read_to_string(&status_file)
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        "running (no status file yet)".to_string()
    };

    // Check if the agent has exceeded its timeout
    if agent_status.contains("running") && is_timed_out(&worktree_dir) {
        agent_status = "timed-out".to_string();
    }

    println!("Agent:     {}", agent);
    println!("Worktree:  {}", worktree_dir.display());
    println!("Status:    {}", agent_status);

    // Show timeout metadata if available
    if let Some(meta) = read_timeout_metadata(&worktree_dir) {
        let hours = meta.timeout_secs / 3600;
        let mins = (meta.timeout_secs % 3600) / 60;
        if hours > 0 {
            println!("Timeout:   {}h{}m", hours, mins);
        } else {
            println!("Timeout:   {}m", mins);
        }
        println!("Started:   {}", meta.started_at);
    }

    // Check tmux session
    let session_name = tmux_session_name(wt_slug);
    if tmux_session_exists(&session_name) {
        println!("tmux:      active ({})", session_name);
    } else {
        println!("tmux:      no active session");
    }

    // Check heartbeat on hub if available
    if let Ok(sync) = crate::sync::SyncManager::new(crosslink_dir) {
        let cache = sync.cache_path();
        // Try both agent ID formats
        for candidate in &[agent.to_string(), format!("driver--{}", wt_slug)] {
            let heartbeat_path = cache.join("agents").join(candidate).join("heartbeat.json");
            if heartbeat_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&heartbeat_path) {
                    if let Ok(hb) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(ts) = hb.get("timestamp").and_then(|v| v.as_str()) {
                            println!("Heartbeat: {}", ts);
                        }
                    }
                }
                break;
            }
        }
    }

    Ok(())
}

/// `crosslink kickoff logs <agent>`
pub fn logs(crosslink_dir: &Path, agent: &str, lines: usize) -> Result<()> {
    // Read the agent's event log from the hub branch
    if let Ok(sync) = crate::sync::SyncManager::new(crosslink_dir) {
        let _ = sync.init_cache();
        let _ = sync.fetch();
        let cache = sync.cache_path();

        // Find agent directory
        let slug = agent.rsplit("--").next().unwrap_or(agent);
        let agents_dir = cache.join("agents");

        let mut found = false;
        if agents_dir.is_dir() {
            for entry in std::fs::read_dir(&agents_dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().to_string();
                if name == agent || name.ends_with(&format!("--{}", slug)) {
                    found = true;
                    println!("Agent: {}", name);

                    // Show heartbeat
                    let hb_path = entry.path().join("heartbeat.json");
                    if hb_path.exists() {
                        let content = std::fs::read_to_string(&hb_path)?;
                        println!("Heartbeat: {}", content.trim());
                    }

                    // Show event log (if CBOR events exist)
                    let events_path = entry.path().join("events.log");
                    if events_path.exists() {
                        let metadata = std::fs::metadata(&events_path)?;
                        println!("Events log: {} bytes", metadata.len());
                    } else {
                        println!("Events log: (none)");
                    }

                    println!();
                    break;
                }
            }
        }

        if !found {
            println!("No agent '{}' found on hub branch.", agent);
            println!("Available agents:");
            if agents_dir.is_dir() {
                for entry in std::fs::read_dir(&agents_dir)? {
                    let entry = entry?;
                    println!("  {}", entry.file_name().to_string_lossy());
                }
            }
        }
    } else {
        bail!("Could not access hub branch. Run 'crosslink sync' first.");
    }

    // Also check local worktree for recent git log
    let root = crosslink_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine repo root"))?;
    let slug = agent.rsplit("--").next().unwrap_or(agent);
    let worktree_dir = root.join(".worktrees").join(slug);

    if worktree_dir.exists() {
        println!("Recent commits in worktree:");
        let output = Command::new("git")
            .current_dir(&worktree_dir)
            .args([
                "log",
                "--oneline",
                &format!("-{}", lines),
                "--format=%h %s (%cr)",
            ])
            .output();

        if let Ok(o) = output {
            if o.status.success() {
                print!("{}", String::from_utf8_lossy(&o.stdout));
            }
        }
    }

    // Suppress unused variable warning
    let _ = lines;

    Ok(())
}

/// `crosslink kickoff stop <agent>`
pub fn stop(_crosslink_dir: &Path, agent: &str, force: bool) -> Result<()> {
    let slug = agent
        .strip_prefix("feature/")
        .or_else(|| agent.strip_prefix("feat-"))
        .unwrap_or(agent);
    let wt_slug = slug.rsplit("--").next().unwrap_or(slug);

    // Try to stop tmux session (local mode)
    let session_name = tmux_session_name(wt_slug);
    if tmux_session_exists(&session_name) {
        let signal = if force { "kill-session" } else { "send-keys" };

        if force {
            let output = Command::new("tmux")
                .args(["kill-session", "-t", &session_name])
                .output()
                .context("Failed to kill tmux session")?;
            if output.status.success() {
                println!("Killed tmux session: {}", session_name);
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("Warning: failed to kill session: {}", stderr.trim());
            }
        } else {
            // Send Ctrl-C gracefully
            let output = Command::new("tmux")
                .args(["send-keys", "-t", &session_name, "C-c", ""])
                .output()
                .context("Failed to send interrupt to tmux session")?;
            if output.status.success() {
                println!("Sent interrupt to tmux session: {}", session_name);
                println!("Use --force to kill immediately.");
            }
        }
        let _ = signal; // consumed in branch logic above
        return Ok(());
    }

    // Try to stop container (docker/podman)
    let container_name = format!("crosslink-agent-{}", agent);
    for runtime in &["docker", "podman"] {
        if command_available(runtime) {
            let stop_cmd = if force { "kill" } else { "stop" };
            let output = Command::new(runtime)
                .args([stop_cmd, &container_name])
                .output();

            if let Ok(o) = output {
                if o.status.success() {
                    println!("Stopped {} container: {}", runtime, container_name);
                    return Ok(());
                }
            }
        }
    }

    bail!(
        "No running agent found for '{}'. Checked tmux session '{}' and container '{}'.",
        agent,
        session_name,
        container_name
    );
}

/// Format a kickoff report as a human-readable table.
pub(crate) fn format_report_table(report: &KickoffReport) -> String {
    let mut out = String::new();
    out.push_str("Kickoff Report");
    if let Some(ref id) = report.agent_id {
        out.push_str(&format!(": {}", id));
    }
    out.push('\n');

    // Metadata line
    let mut meta = Vec::new();
    if let Some(id) = report.issue_id {
        meta.push(format!("Issue: #{}", id));
    }
    if let Some(ref s) = report.status {
        meta.push(format!("Status: {}", s));
    }
    if let Some(ref phases) = report.phases {
        let total: u64 = [
            &phases.exploration,
            &phases.planning,
            &phases.implementation,
            &phases.testing,
            &phases.validation,
            &phases.review,
        ]
        .iter()
        .filter_map(|p| p.as_ref().map(|t| t.duration_s))
        .sum();
        if total > 0 {
            meta.push(format!("Duration: {}", format_duration(total)));
        }
    }
    if !meta.is_empty() {
        out.push_str(&meta.join(" | "));
        out.push('\n');
    }
    out.push('\n');

    // Phase timing
    if let Some(ref phases) = report.phases {
        out.push_str("Phase Timing:\n");
        let phase_list: &[(&str, &Option<PhaseTiming>)] = &[
            ("exploration", &phases.exploration),
            ("planning", &phases.planning),
            ("implementation", &phases.implementation),
            ("testing", &phases.testing),
            ("validation", &phases.validation),
            ("review", &phases.review),
        ];
        for (name, timing) in phase_list {
            if let Some(t) = timing {
                out.push_str(&format_phase_line(name, t));
            }
        }
        out.push('\n');
    }

    // Criteria
    if !report.criteria.is_empty() {
        out.push_str("Acceptance Criteria:\n");
        for c in &report.criteria {
            let symbol = match c.verdict.as_str() {
                "pass" => "\u{2713}",
                "partial" => "~",
                "fail" => "\u{2717}",
                "not_applicable" => "-",
                _ => "?",
            };
            out.push_str(&format!("  {} {}  {}\n", symbol, c.id, c.evidence));
        }
        out.push('\n');
        let s = &report.summary;
        out.push_str(&format!(
            "{} criteria: {} pass, {} partial, {} fail",
            s.total, s.pass, s.partial, s.fail
        ));
        if s.not_applicable > 0 {
            out.push_str(&format!(", {} n/a", s.not_applicable));
        }
        if s.needs_clarification > 0 {
            out.push_str(&format!(", {} unclear", s.needs_clarification));
        }
        out.push('\n');
    }

    // Files and commits
    if let Some(ref files) = report.files_changed {
        if !files.is_empty() {
            out.push_str(&format!("\nFiles changed: {}\n", files.join(", ")));
        }
    }
    if let Some(ref commits) = report.commits {
        if !commits.is_empty() {
            out.push_str(&format!("Commits: {}\n", commits.join(", ")));
        }
    }

    out
}

/// Format a kickoff report as PR-ready markdown.
pub(crate) fn format_report_markdown(report: &KickoffReport) -> String {
    let mut out = String::new();
    out.push_str("## Kickoff Report\n\n");

    // Metadata
    if let Some(ref id) = report.agent_id {
        out.push_str(&format!("**Agent**: {}\n", id));
    }
    if let Some(id) = report.issue_id {
        out.push_str(&format!("**Issue**: #{}\n", id));
    }
    if let Some(ref s) = report.status {
        out.push_str(&format!("**Status**: {}\n", s));
    }
    out.push('\n');

    // Criteria table
    if !report.criteria.is_empty() {
        out.push_str("| ID | Verdict | Evidence |\n");
        out.push_str("|---|---|---|\n");
        for c in &report.criteria {
            let verdict_display = match c.verdict.as_str() {
                "pass" => "\u{2713} pass",
                "partial" => "~ partial",
                "fail" => "\u{2717} fail",
                "not_applicable" => "- n/a",
                "needs_clarification" => "? unclear",
                _ => &c.verdict,
            };
            let evidence = c.evidence.replace('|', "\\|");
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                c.id, verdict_display, evidence
            ));
        }
        out.push('\n');
        let s = &report.summary;
        out.push_str(&format!(
            "**{} criteria**: {} pass, {} partial, {} fail\n",
            s.total, s.pass, s.partial, s.fail
        ));
    }

    out
}

/// Format an aggregated summary of all agent reports.
pub(crate) fn format_report_all_table(reports: &[(&str, KickoffReport)]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Agent Kickoff Summary ({} agents)\n\n",
        reports.len()
    ));
    out.push_str(&format!(
        "{:<32} {:<12} {:<10} {:<14} {}\n",
        "Agent", "Status", "Tests", "Criteria", "Duration"
    ));

    let mut completed = 0u32;
    let mut failed = 0u32;

    for (slug, r) in reports {
        let status = r.status.as_deref().unwrap_or("unknown");
        match status {
            "completed" => completed += 1,
            "failed" => failed += 1,
            _ => {}
        }

        // Tests
        let tests = if let Some(ref phases) = r.phases {
            if let Some(ref t) = phases.testing {
                let run = t.tests_run.unwrap_or(0);
                let passed = t.tests_passed.unwrap_or(0);
                format!("{}/{}", passed, run)
            } else {
                "-".to_string()
            }
        } else {
            "-".to_string()
        };

        // Criteria
        let criteria_str = if r.summary.total > 0 {
            format!("{}/{} pass", r.summary.pass, r.summary.total)
        } else {
            "-".to_string()
        };

        // Duration
        let duration = if let Some(ref phases) = r.phases {
            let total: u64 = [
                &phases.exploration,
                &phases.planning,
                &phases.implementation,
                &phases.testing,
                &phases.validation,
                &phases.review,
            ]
            .iter()
            .filter_map(|p| p.as_ref().map(|t| t.duration_s))
            .sum();
            if total > 0 {
                format_duration(total)
            } else {
                "-".to_string()
            }
        } else {
            "-".to_string()
        };

        out.push_str(&format!(
            "{:<32} {:<12} {:<10} {:<14} {}\n",
            slug, status, tests, criteria_str, duration
        ));
    }

    out.push_str(&format!(
        "\nTotal: {} completed, {} failed\n",
        completed, failed
    ));
    out
}

/// Display the spec validation report for a kickoff agent.
pub fn report(crosslink_dir: &Path, agent: &str, format: ReportFormat) -> Result<()> {
    let root = crosslink_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine repo root"))?;

    let slug = agent
        .strip_prefix("feature/")
        .or_else(|| agent.strip_prefix("feat-"))
        .unwrap_or(agent);
    let wt_slug = slug.rsplit("--").next().unwrap_or(slug);

    let worktree_dir = root.join(".worktrees").join(wt_slug);
    if !worktree_dir.exists() {
        bail!(
            "No worktree found for '{}'. Checked: {}",
            agent,
            worktree_dir.display()
        );
    }

    let report_file = worktree_dir.join(".kickoff-report.json");
    if !report_file.exists() {
        let status_file = worktree_dir.join(".kickoff-status");
        let status = if status_file.exists() {
            std::fs::read_to_string(&status_file)
                .unwrap_or_default()
                .trim()
                .to_string()
        } else {
            "still running".to_string()
        };
        bail!(
            "No validation report found for '{}'. Agent status: {}",
            agent,
            status
        );
    }

    let content =
        std::fs::read_to_string(&report_file).context("Failed to read .kickoff-report.json")?;

    match format {
        ReportFormat::Json => {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&parsed).unwrap_or(content)
                );
            } else {
                print!("{}", content);
            }
        }
        ReportFormat::Table => {
            let r: KickoffReport =
                serde_json::from_str(&content).context("Failed to parse .kickoff-report.json")?;
            for w in validate_kickoff_report(&r) {
                eprintln!("Warning: {}", w);
            }
            print!("{}", format_report_table(&r));
        }
        ReportFormat::Markdown => {
            let r: KickoffReport =
                serde_json::from_str(&content).context("Failed to parse .kickoff-report.json")?;
            print!("{}", format_report_markdown(&r));
        }
    }

    Ok(())
}

/// Display aggregated reports from all agent worktrees.
pub fn report_all(crosslink_dir: &Path, format: ReportFormat) -> Result<()> {
    let root = crosslink_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine repo root"))?;

    let worktrees_dir = root.join(".worktrees");
    if !worktrees_dir.is_dir() {
        bail!("No .worktrees directory found");
    }

    let mut reports: Vec<(String, KickoffReport)> = Vec::new();

    for entry in std::fs::read_dir(&worktrees_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let report_file = entry.path().join(".kickoff-report.json");
        if !report_file.exists() {
            continue;
        }
        let content = match std::fs::read_to_string(&report_file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let r: KickoffReport = match serde_json::from_str(&content) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let slug = entry.file_name().to_string_lossy().to_string();
        reports.push((slug, r));
    }

    if reports.is_empty() {
        bail!("No kickoff reports found in any worktree");
    }

    match format {
        ReportFormat::Json => {
            let json_reports: Vec<_> = reports.iter().map(|(_, r)| r).collect();
            println!("{}", serde_json::to_string_pretty(&json_reports)?);
        }
        ReportFormat::Table => {
            let refs: Vec<(&str, KickoffReport)> = reports
                .iter()
                .map(|(s, r)| (s.as_str(), r.clone()))
                .collect();
            print!("{}", format_report_all_table(&refs));
        }
        ReportFormat::Markdown => {
            let refs: Vec<(&str, KickoffReport)> = reports
                .iter()
                .map(|(s, r)| (s.as_str(), r.clone()))
                .collect();
            print!("{}", format_report_all_table(&refs));
        }
    }

    Ok(())
}
