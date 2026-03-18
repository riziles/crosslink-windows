// Cleanup and list commands: discover agents, classify, remove stale artifacts.

use anyhow::Result;
use serde::Serialize;
use std::path::Path;
use std::process::Command;

use super::helpers::{
    command_available, is_timed_out, normalize_status, read_agent_id, read_agent_issue,
    tmux_session_exists, tmux_session_name, truncate_str,
};
use super::types::{AgentInfo, CleanupClass, CleanupResult};

/// Discover all kickoff agents by scanning worktrees, tmux sessions, and Docker containers.
///
/// Shared discovery logic used by both `list` and `cleanup`.
pub(crate) fn discover_agents(crosslink_dir: &Path) -> Result<Vec<AgentInfo>> {
    let root = crosslink_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine repo root"))?;

    let worktrees_dir = root.join(".worktrees");

    let mut agents: Vec<AgentInfo> = Vec::new();

    // --- Source 1: Worktree scan ---
    if worktrees_dir.is_dir() {
        for entry in std::fs::read_dir(&worktrees_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().to_string();
            let wt_path = entry.path();

            // Read .kickoff-status sentinel
            let status_file = wt_path.join(".kickoff-status");
            let agent_status = if status_file.exists() {
                let raw = std::fs::read_to_string(&status_file)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                normalize_status(&raw)
            } else {
                "running".to_string()
            };

            // Try to read issue from .kickoff-criteria.json or agent config
            let issue = read_agent_issue(&wt_path, crosslink_dir);

            // Derive agent ID from agent config if available
            let agent_id = read_agent_id(&wt_path, crosslink_dir)
                .unwrap_or_else(|| format!("driver--{}", dir_name));

            // Check tmux session
            let session_name = tmux_session_name(&dir_name);
            let tmux_active = tmux_session_exists(&session_name);

            // Reconcile status: check timeout, then tmux liveness
            let final_status = if agent_status == "running" && is_timed_out(&wt_path) {
                "timed-out".to_string()
            } else if agent_status == "running" && !tmux_active {
                // Check if there's a docker container instead (handled below as overlay)
                "stopped".to_string()
            } else {
                agent_status
            };

            agents.push(AgentInfo {
                id: agent_id,
                issue,
                status: final_status,
                session: if tmux_active {
                    Some(session_name)
                } else {
                    None
                },
                worktree: wt_path.to_string_lossy().to_string(),
                docker: None,
            });
        }
    }

    // --- Source 2: Docker containers ---
    if command_available("docker") {
        if let Ok(output) = Command::new("docker")
            .args([
                "ps",
                "-a",
                "--filter",
                "label=crosslink-agent=true",
                "--format",
                "{{.Names}}\t{{.Status}}\t{{.Label \"crosslink-task\"}}",
            ])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if parts.len() >= 2 {
                        let container_name = parts[0];
                        let container_status_raw = parts[1];
                        let task_label = parts.get(2).unwrap_or(&"");

                        // Try to match to an existing worktree agent
                        let matched = agents.iter_mut().find(|a| {
                            // Match by task label containing the worktree dir name
                            if !task_label.is_empty() {
                                a.worktree.contains(task_label)
                            } else {
                                // Match by container name containing the agent slug
                                let slug = a.id.rsplit("--").next().unwrap_or(&a.id);
                                container_name.contains(slug)
                            }
                        });

                        if let Some(agent) = matched {
                            agent.docker = Some(container_name.to_string());
                            // If container is running, override status
                            if container_status_raw.starts_with("Up") && agent.status == "stopped" {
                                agent.status = "running".to_string();
                            }
                        } else {
                            // Docker-only agent (no worktree found)
                            let docker_status = if container_status_raw.starts_with("Up") {
                                "running"
                            } else if container_status_raw.contains("Exited (0)") {
                                "done"
                            } else {
                                "failed"
                            };
                            agents.push(AgentInfo {
                                id: container_name.to_string(),
                                issue: if task_label.is_empty() {
                                    None
                                } else {
                                    Some(task_label.to_string())
                                },
                                status: docker_status.to_string(),
                                session: None,
                                worktree: String::new(),
                                docker: Some(container_name.to_string()),
                            });
                        }
                    }
                }
            }
        }
    }

    Ok(agents)
}

/// `crosslink kickoff list`
///
/// Enumerate all kickoff agents by scanning worktrees, tmux sessions, and Docker containers.
pub fn list(crosslink_dir: &Path, status_filter: &str, json: bool, quiet: bool) -> Result<()> {
    let agents = discover_agents(crosslink_dir)?;

    // --- Filter by status ---
    let filtered: Vec<&AgentInfo> = if status_filter == "all" {
        agents.iter().collect()
    } else {
        agents
            .iter()
            .filter(|a| a.status == status_filter)
            .collect()
    };

    // --- Output ---
    if quiet {
        for agent in &filtered {
            println!("{}", agent.id);
        }
        return Ok(());
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&filtered)?);
        return Ok(());
    }

    if filtered.is_empty() {
        println!("No kickoff agents found.");
        return Ok(());
    }

    // Table output
    println!(
        "{:<36} {:<8} {:<10} {:<24} WORKTREE",
        "ID", "ISSUE", "STATUS", "SESSION"
    );
    for agent in &filtered {
        let issue_display = agent.issue.as_deref().unwrap_or("-");
        let session_display = agent.session.as_deref().unwrap_or("-");
        let worktree_display = if agent.worktree.is_empty() {
            "-"
        } else {
            // Show just the leaf directory name for brevity
            std::path::Path::new(&agent.worktree)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&agent.worktree)
        };
        // Append docker indicator if present
        let status_display = if agent.docker.is_some() {
            format!("{} \u{1f433}", agent.status)
        } else {
            agent.status.clone()
        };
        println!(
            "{:<36} {:<8} {:<10} {:<24} {}",
            truncate_str(&agent.id, 35),
            truncate_str(issue_display, 7),
            status_display,
            truncate_str(session_display, 23),
            worktree_display
        );
    }

    Ok(())
}

/// Classify an agent for cleanup purposes.
fn classify_agent(agent: &AgentInfo) -> CleanupClass {
    match agent.status.as_str() {
        "done" => CleanupClass::Done,
        "running" => CleanupClass::Active,
        // "stopped" means tmux/container gone but no DONE sentinel — potentially stale
        "stopped" => CleanupClass::Stale,
        // "failed" agents are safe to clean up (they have a terminal sentinel)
        "failed" => CleanupClass::Done,
        // "timed-out" agents exceeded their timeout budget — treat as stale
        "timed-out" => CleanupClass::Stale,
        _ => CleanupClass::Stale,
    }
}

/// `crosslink kickoff cleanup`
///
/// Discover and remove stale kickoff agent artifacts: completed tmux sessions,
/// worktrees with DONE sentinels, and orphaned worktrees whose sessions no longer exist.
pub fn cleanup(
    crosslink_dir: &Path,
    dry_run: bool,
    force: bool,
    keep: usize,
    json_output: bool,
) -> Result<()> {
    let agents = discover_agents(crosslink_dir)?;

    // Classify each agent
    let candidates: Vec<(AgentInfo, CleanupClass)> = agents
        .into_iter()
        .map(|a| {
            let class = classify_agent(&a);
            (a, class)
        })
        .collect();

    // Separate active agents (never cleaned) from removable ones
    let (active, removable): (Vec<_>, Vec<_>) = candidates
        .into_iter()
        .partition(|(_, class)| *class == CleanupClass::Active);

    // Without --force, only clean Done agents (not Stale)
    let (mut to_clean, skipped_stale): (Vec<_>, Vec<_>) = if force {
        (removable, vec![])
    } else {
        removable
            .into_iter()
            .partition(|(_, class)| *class == CleanupClass::Done)
    };

    // Sort by worktree path (as a proxy for creation order) so --keep works predictably
    to_clean.sort_by(|a, b| a.0.worktree.cmp(&b.0.worktree));

    // Apply --keep: keep the N most recent (last N items after sorting)
    let to_clean = if keep > 0 && to_clean.len() > keep {
        to_clean[..to_clean.len() - keep].to_vec()
    } else if keep > 0 && to_clean.len() <= keep {
        vec![] // keep all
    } else {
        to_clean
    };

    // --- Dry-run / JSON output ---
    if json_output {
        #[derive(Serialize)]
        struct CleanupPlan {
            to_clean: Vec<CleanupPlanEntry>,
            skipped_stale: Vec<CleanupPlanEntry>,
            active: Vec<CleanupPlanEntry>,
            dry_run: bool,
        }
        #[derive(Serialize)]
        struct CleanupPlanEntry {
            id: String,
            status: String,
            class: CleanupClass,
            worktree: String,
            session: Option<String>,
            docker: Option<String>,
        }
        let to_entry = |items: &[(AgentInfo, CleanupClass)]| -> Vec<CleanupPlanEntry> {
            items
                .iter()
                .map(|(a, c)| CleanupPlanEntry {
                    id: a.id.clone(),
                    status: a.status.clone(),
                    class: c.clone(),
                    worktree: a.worktree.clone(),
                    session: a.session.clone(),
                    docker: a.docker.clone(),
                })
                .collect()
        };
        let plan = CleanupPlan {
            to_clean: to_entry(&to_clean),
            skipped_stale: to_entry(&skipped_stale),
            active: to_entry(&active),
            dry_run,
        };
        println!("{}", serde_json::to_string_pretty(&plan)?);
        if dry_run {
            return Ok(());
        }
    }

    if to_clean.is_empty() && skipped_stale.is_empty() {
        if !json_output {
            println!("No agents to clean up.");
        }
        return Ok(());
    }

    if dry_run || !json_output {
        // Print the plan
        if !to_clean.is_empty() {
            println!("Cleanup candidates:\n");
            for (agent, class) in &to_clean {
                let class_label = match class {
                    CleanupClass::Done => "DONE  ",
                    CleanupClass::Stale => "STALE ",
                    _ => "      ",
                };
                let wt_display = if agent.worktree.is_empty() {
                    "-".to_string()
                } else {
                    std::path::Path::new(&agent.worktree)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&agent.worktree)
                        .to_string()
                };
                let session_info = agent
                    .session
                    .as_deref()
                    .map(|s| format!("tmux: {}", s))
                    .unwrap_or_else(|| "tmux: exited".to_string());
                let docker_info = agent
                    .docker
                    .as_deref()
                    .map(|d| format!("  docker: {}", d))
                    .unwrap_or_default();
                println!(
                    "  {}  {:<40} worktree: {:<30} {}{}",
                    class_label, agent.id, wt_display, session_info, docker_info
                );
            }
        }

        if !skipped_stale.is_empty() {
            println!(
                "\n{} stale agent(s) skipped (use --force to include):",
                skipped_stale.len()
            );
            for (agent, _) in &skipped_stale {
                let wt_display = if agent.worktree.is_empty() {
                    "-".to_string()
                } else {
                    std::path::Path::new(&agent.worktree)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&agent.worktree)
                        .to_string()
                };
                println!("  STALE  {:<40} worktree: {}", agent.id, wt_display);
            }
        }

        if dry_run {
            let wt_count = to_clean
                .iter()
                .filter(|(a, _)| !a.worktree.is_empty())
                .count();
            let tmux_count = to_clean.iter().filter(|(a, _)| a.session.is_some()).count();
            let docker_count = to_clean.iter().filter(|(a, _)| a.docker.is_some()).count();
            println!();
            print!("Would remove {} worktree(s)", wt_count);
            if tmux_count > 0 {
                print!(", kill {} tmux session(s)", tmux_count);
            }
            if docker_count > 0 {
                print!(", remove {} container(s)", docker_count);
            }
            println!(".");
            println!("Run without --dry-run to proceed.");
            return Ok(());
        }

        println!();
    }

    // --- Execute cleanup ---
    let mut results: Vec<CleanupResult> = Vec::new();

    for (agent, class) in &to_clean {
        let mut result = CleanupResult {
            id: agent.id.clone(),
            class: class.clone(),
            worktree_removed: false,
            tmux_killed: false,
            container_removed: false,
            error: None,
        };

        // 1. Kill tmux session if it still exists
        if let Some(ref session_name) = agent.session {
            match Command::new("tmux")
                .args(["kill-session", "-t", session_name])
                .output()
            {
                Ok(o) if o.status.success() => {
                    result.tmux_killed = true;
                    if !json_output {
                        println!("  Killed tmux session: {}", session_name);
                    }
                }
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    eprintln!(
                        "  Warning: failed to kill tmux session {}: {}",
                        session_name,
                        stderr.trim()
                    );
                }
                Err(e) => {
                    eprintln!("  Warning: tmux error for {}: {}", session_name, e);
                }
            }
        }

        // 2. Remove Docker/Podman container if present
        if let Some(ref container_name) = agent.docker {
            for runtime in &["docker", "podman"] {
                if command_available(runtime) {
                    if let Ok(o) = Command::new(runtime)
                        .args(["rm", "-f", container_name])
                        .output()
                    {
                        if o.status.success() {
                            result.container_removed = true;
                            if !json_output {
                                println!("  Removed {} container: {}", runtime, container_name);
                            }
                            break;
                        }
                    }
                }
            }
        }

        // 3. Remove the git worktree
        if !agent.worktree.is_empty() && std::path::Path::new(&agent.worktree).exists() {
            match Command::new("git")
                .args(["worktree", "remove", "--force", &agent.worktree])
                .output()
            {
                Ok(o) if o.status.success() => {
                    result.worktree_removed = true;
                    let wt_display = std::path::Path::new(&agent.worktree)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&agent.worktree);
                    if !json_output {
                        println!("  Removed worktree: {}", wt_display);
                    }
                }
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    let msg = format!("git worktree remove failed: {}", stderr.trim());
                    eprintln!("  Warning: {}", msg);
                    result.error = Some(msg);
                }
                Err(e) => {
                    let msg = format!("git worktree remove error: {}", e);
                    eprintln!("  Warning: {}", msg);
                    result.error = Some(msg);
                }
            }
        }

        results.push(result);
    }

    // --- Summary ---
    if json_output {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        let wt_removed = results.iter().filter(|r| r.worktree_removed).count();
        let tmux_killed = results.iter().filter(|r| r.tmux_killed).count();
        let containers_removed = results.iter().filter(|r| r.container_removed).count();
        let errors = results.iter().filter(|r| r.error.is_some()).count();

        println!();
        print!("Cleaned up {} agent(s)", results.len());
        if wt_removed > 0 {
            print!(": {} worktree(s)", wt_removed);
        }
        if tmux_killed > 0 {
            print!(", {} tmux session(s)", tmux_killed);
        }
        if containers_removed > 0 {
            print!(", {} container(s)", containers_removed);
        }
        if errors > 0 {
            print!(" ({} error(s))", errors);
        }
        println!(".");
    }

    Ok(())
}
