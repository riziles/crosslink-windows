// Launch logic: worktree creation, agent initialization, tmux/container launch,
// and watchdog sidecar.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::identity::AgentConfig;

use super::helpers::{command_available, read_watchdog_config, tmux_session_exists};
use super::prompt::build_agent_command;
use super::types::{ContainerMode, WatchdogConfig};

/// Create a feature branch and worktree for the agent.
pub(crate) fn create_worktree(
    repo_root: &Path,
    slug: &str,
    base_branch: Option<&str>,
) -> Result<(std::path::PathBuf, String)> {
    let branch_name = format!("feature/{}", slug);
    let worktree_dir = repo_root.join(".worktrees").join(slug);

    if worktree_dir.exists() {
        bail!(
            "Worktree already exists at {}. Remove it first or use --branch to target an existing branch.",
            worktree_dir.display()
        );
    }

    // Determine base ref
    let base = base_branch.unwrap_or("HEAD");

    // Create the worktree with a new branch
    let output = Command::new("git")
        .current_dir(repo_root)
        .args(["worktree", "add", "-b", &branch_name])
        .arg(&worktree_dir)
        .arg(base)
        .output()
        .context("Failed to create git worktree")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to create worktree: {}", stderr.trim());
    }

    Ok((worktree_dir, branch_name))
}

/// Initialize crosslink and agent identity in the worktree.
pub(crate) fn init_worktree_agent(
    worktree_dir: &Path,
    crosslink_dir: &Path,
    slug: &str,
) -> Result<String> {
    // Run crosslink init --force in the worktree
    let output = Command::new("crosslink")
        .current_dir(worktree_dir)
        .args(["init", "--force", "--skip-signing", "--defaults"])
        .output()
        .context("Failed to run crosslink init in worktree")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("Warning: crosslink init in worktree: {}", stderr.trim());
    }

    // Derive agent ID from parent agent or hostname
    let parent_id = AgentConfig::load(crosslink_dir)?
        .map(|c| c.agent_id)
        .unwrap_or_else(|| "driver".to_string());

    let agent_id = format!("{}--{}", parent_id, slug);

    // Initialize agent identity in worktree (skip key gen — inherits from parent)
    let wt_crosslink = worktree_dir.join(".crosslink");
    if wt_crosslink.exists() {
        // Only init if not already configured
        if AgentConfig::load(&wt_crosslink)?.is_none() {
            let _ = super::super::agent::init(
                &wt_crosslink,
                &agent_id,
                Some(&format!("Kickoff agent for: {}", slug)),
                true, // no-key: inherit parent's key
                false,
            );

            // Copy parent's SSH key info into the new agent config and publish
            // the key under the new agent ID so `crosslink trust approve` can find it.
            if let Some(parent_config) = AgentConfig::load(crosslink_dir)? {
                if let Some(ref public_key) = parent_config.ssh_public_key {
                    if let Ok(Some(mut child_config)) = AgentConfig::load(&wt_crosslink) {
                        child_config.ssh_key_path = parent_config.ssh_key_path.clone();
                        child_config.ssh_fingerprint = parent_config.ssh_fingerprint.clone();
                        child_config.ssh_public_key = Some(public_key.clone());

                        let agent_json = wt_crosslink.join("agent.json");
                        if let Ok(json) = serde_json::to_string_pretty(&child_config) {
                            let _ = std::fs::write(&agent_json, json);
                        }

                        // Publish the parent's public key under the new agent ID
                        if let Err(e) = super::super::trust::publish_agent_key(
                            &wt_crosslink,
                            &agent_id,
                            public_key,
                        ) {
                            eprintln!(
                                "Warning: Could not publish key for agent '{}': {}",
                                agent_id, e
                            );
                            eprintln!("Key will be auto-published on next `crosslink sync`.");
                        }
                    }
                }
            }
        }
    }

    // Sync coordination state
    let output = Command::new("crosslink")
        .current_dir(worktree_dir)
        .args(["sync"])
        .output();

    if let Ok(o) = output {
        if !o.status.success() {
            eprintln!("Warning: crosslink sync in worktree returned non-zero");
        }
    }

    Ok(agent_id)
}

/// Exclude kickoff files from git tracking.
pub(crate) fn exclude_kickoff_files(worktree_dir: &Path) -> Result<()> {
    let output = Command::new("git")
        .current_dir(worktree_dir)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .context("Failed to get git common dir")?;

    let common_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let exclude_path = std::path::PathBuf::from(&common_dir).join("info/exclude");

    // Ensure parent directory exists
    if let Some(parent) = exclude_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let existing = std::fs::read_to_string(&exclude_path).unwrap_or_default();
    let additions = super::types::missing_exclude_patterns(&existing);

    if !additions.is_empty() {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&exclude_path)
            .context("Failed to open git exclude file")?;
        for pattern in additions {
            writeln!(file, "{}", pattern)?;
        }
    }

    Ok(())
}

/// Build the watchdog shell script that monitors heartbeat staleness and
/// nudges idle agents by sending "continue" via tmux send-keys.
fn build_watchdog_script(session_name: &str, worktree_dir: &Path, cfg: &WatchdogConfig) -> String {
    // Use portable stat command — try GNU stat first, fall back to BSD
    format!(
        r#"NUDGES=0
sleep {grace}
while true; do
    sleep {interval}
    if [ -f "{worktree}/.kickoff-status" ]; then exit 0; fi
    if ! tmux has-session -t "{session}" 2>/dev/null; then exit 0; fi
    HB="{worktree}/.crosslink/.cache/last-heartbeat"
    if [ -f "$HB" ]; then
        LAST=$(stat -c %Y "$HB" 2>/dev/null || stat -f %m "$HB" 2>/dev/null)
        NOW=$(date +%s)
        AGE=$((NOW - LAST))
        if [ "$AGE" -gt {staleness} ]; then
            if [ "$NUDGES" -ge {max_nudges} ]; then exit 1; fi
            NUDGES=$((NUDGES + 1))
            tmux send-keys -t "{session}" "continue working, the task is not yet complete" Enter
        fi
    fi
done
"#,
        grace = cfg.grace_period_secs,
        interval = cfg.check_interval_secs,
        worktree = worktree_dir.display(),
        session = session_name,
        staleness = cfg.staleness_secs,
        max_nudges = cfg.max_nudges,
    )
}

/// Spawn a background watchdog process that monitors the agent's heartbeat
/// and sends "continue" to the tmux session if the agent goes idle.
fn spawn_watchdog(session_name: &str, worktree_dir: &Path, cfg: &WatchdogConfig) -> Result<()> {
    let script = build_watchdog_script(session_name, worktree_dir, cfg);

    Command::new("bash")
        .args(["-c", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Failed to spawn watchdog process")?;

    Ok(())
}

/// Launch the agent as a local tmux process.
#[allow(clippy::too_many_arguments)]
pub(crate) fn launch_local(
    worktree_dir: &Path,
    session_name: &str,
    model: &str,
    allowed_tools: &str,
    timeout: Duration,
    timeout_cmd: &str,
    sandbox_command: Option<&str>,
    crosslink_dir: &Path,
    skip_permissions: bool,
) -> Result<()> {
    // Create the tmux session
    let output = Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            session_name,
            "-c",
            &worktree_dir.to_string_lossy(),
        ])
        .output()
        .context("Failed to create tmux session")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to create tmux session: {}", stderr.trim());
    }

    // Build the claude command (with optional sandbox wrapping)
    let cmd = build_agent_command(
        timeout_cmd,
        timeout.as_secs(),
        model,
        allowed_tools,
        "KICKOFF.md",
        sandbox_command,
        worktree_dir,
        skip_permissions,
    );

    // Write initial status sentinel BEFORE sending the command.
    // This ensures we never have a worktree in limbo with no status.
    std::fs::write(worktree_dir.join(".kickoff-status"), "LAUNCHING\n")
        .context("Failed to write initial .kickoff-status")?;

    // Send the command to the tmux session
    let output = Command::new("tmux")
        .args(["send-keys", "-t", session_name, &cmd, "Enter"])
        .output()
        .context("Failed to send command to tmux session")?;

    if !output.status.success() {
        // Mark as failed before bailing
        let _ = std::fs::write(worktree_dir.join(".kickoff-status"), "FAILED\n");
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to send keys to tmux: {}", stderr.trim());
    }

    // Update status to RUNNING now that the command has been sent
    let _ = std::fs::write(worktree_dir.join(".kickoff-status"), "RUNNING\n");

    // Spawn watchdog sidecar to nudge idle agents
    let watchdog_cfg = read_watchdog_config(crosslink_dir);
    if watchdog_cfg.enabled {
        if let Err(e) = spawn_watchdog(session_name, worktree_dir, &watchdog_cfg) {
            eprintln!("Warning: failed to spawn watchdog: {}", e);
        }
    }

    Ok(())
}

/// Launch the agent in a Docker or Podman container.
pub(crate) fn launch_container(
    runtime: &ContainerMode,
    worktree_dir: &Path,
    image: &str,
    agent_id: &str,
    model: &str,
    allowed_tools: &str,
    timeout: Duration,
) -> Result<String> {
    let runtime_cmd = match runtime {
        ContainerMode::Docker => "docker",
        ContainerMode::Podman => "podman",
        ContainerMode::None => unreachable!(),
    };

    // Check runtime is available
    if !command_available(runtime_cmd) {
        bail!(
            "{} is not installed. Install it or use --container none for local mode.",
            runtime_cmd
        );
    }

    let timeout_secs = timeout.as_secs();
    let container_name = format!("crosslink-agent-{}", agent_id);

    // Resolve host auth path for credential mounting
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let host_auth = format!("{}/.claude", home);

    // Get host UID/GID for remapping (skip on Windows — Docker Desktop handles user mapping)
    let uid_gid = if cfg!(target_os = "windows") {
        None
    } else {
        let uid = Command::new("id")
            .arg("-u")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "1000".to_string());
        let gid = Command::new("id")
            .arg("-g")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "1000".to_string());
        Some((uid, gid))
    };

    let mut args = vec![
        "run".to_string(),
        "-d".to_string(),
        "--name".to_string(),
        container_name.clone(),
        // Hard-kill the container after the timeout (grace period = 10s on top)
        "--stop-timeout".to_string(),
        format!("{}", timeout_secs),
        // Mount the worktree as workspace
        "-v".to_string(),
        format!("{}:/workspaces/repo", worktree_dir.to_string_lossy()),
        // Mount credentials read-only
        "-v".to_string(),
        format!("{}:/host-auth:ro", host_auth),
        // Environment
        "-e".to_string(),
        format!("AGENT_ID={}", agent_id),
    ];

    // Pass UID/GID to container for user remapping (non-Windows only)
    if let Some((uid, gid)) = &uid_gid {
        args.extend([
            "-e".to_string(),
            format!("HOST_UID={}", uid),
            "-e".to_string(),
            format!("HOST_GID={}", gid),
        ]);
    }

    // Image and command
    args.push(image.to_string());
    args.push("bash".to_string());
    args.push("-c".to_string());
    args.push(format!(
        "cd /workspaces/repo && timeout {}s claude --model {} --allowedTools '{}' -- \"$(cat KICKOFF.md)\"",
        timeout_secs, model, allowed_tools
    ));

    let output = Command::new(runtime_cmd)
        .args(&args)
        .output()
        .with_context(|| format!("Failed to launch {} container", runtime_cmd))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{} container launch failed: {}", runtime_cmd, stderr.trim());
    }

    let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(container_id)
}

/// Launch the plan agent in a tmux session using the given preflight result.
pub(crate) fn launch_plan_in_tmux(
    worktree_dir: &Path,
    session_name: &str,
    cmd: &str,
    crosslink_dir: &Path,
) -> Result<()> {
    let output = Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            session_name,
            "-c",
            &worktree_dir.to_string_lossy(),
        ])
        .output()
        .context("Failed to create tmux session")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to create tmux session: {}", stderr.trim());
    }

    let output = Command::new("tmux")
        .args(["send-keys", "-t", session_name, cmd, "Enter"])
        .output()
        .context("Failed to send command to tmux session")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to send keys to tmux: {}", stderr.trim());
    }

    // Spawn watchdog sidecar to nudge idle agents
    let watchdog_cfg = read_watchdog_config(crosslink_dir);
    if watchdog_cfg.enabled {
        if let Err(e) = spawn_watchdog(session_name, worktree_dir, &watchdog_cfg) {
            eprintln!("Warning: failed to spawn watchdog: {}", e);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_watchdog_script_contains_key_elements() {
        let cfg = WatchdogConfig {
            enabled: true,
            staleness_secs: 300,
            max_nudges: 3,
            check_interval_secs: 60,
            grace_period_secs: 120,
        };
        let script = build_watchdog_script("feat-my-agent", Path::new("/tmp/wt"), &cfg);
        assert!(script.contains("sleep 120")); // grace period
        assert!(script.contains("sleep 60")); // check interval
        assert!(script.contains(".kickoff-status"));
        assert!(script.contains("feat-my-agent"));
        assert!(script.contains("last-heartbeat"));
        assert!(script.contains("continue working"));
        assert!(script.contains("NUDGES"));
        assert!(script.contains("-gt 300")); // staleness threshold
        assert!(script.contains("-ge 3")); // max nudges
    }
}
