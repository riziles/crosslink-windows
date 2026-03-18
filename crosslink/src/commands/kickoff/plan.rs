// Plan mode: gap analysis and show-plan commands.

use anyhow::{bail, Context, Result};
use std::path::Path;

use crate::db::Database;
use crate::identity::AgentConfig;

use super::helpers::{
    preflight_check, rand_hex_suffix, rand_suffix, repo_root, slugify, tmux_session_name,
    tmux_session_exists,
};
use super::launch::{create_worktree, exclude_kickoff_files, init_worktree_agent, launch_plan_in_tmux};
use super::prompt::{build_agent_command, build_allowed_tools_plan, build_plan_prompt};
use super::types::{ContainerMode, PlanOpts, VerifyLevel};

/// Main entry point: `crosslink kickoff plan`.
pub fn plan(crosslink_dir: &Path, db: &Database, opts: &PlanOpts) -> Result<()> {
    // Plan mode always uses tmux — reject on Windows early.
    if cfg!(target_os = "windows") && !opts.dry_run {
        bail!(
            "Plan mode requires tmux, which is not available on Windows.\n\
             Use `--container docker` for agent kickoff on Windows."
        );
    }

    // 1. Pre-flight: validate all required external commands
    let preflight = if !opts.dry_run {
        Some(preflight_check(
            &ContainerMode::None,
            &VerifyLevel::Local,
            crosslink_dir,
        )?)
    } else {
        None
    };

    let root = repo_root()?;
    let title_slug = if opts.doc.title.is_empty() {
        "analysis".to_string()
    } else {
        slugify(&opts.doc.title)
    };
    let slug = format!("plan-{}-{}", title_slug, rand_hex_suffix());

    // 2. Create or find issue (optional for plan mode)
    let issue_id = if let Some(id) = opts.issue {
        if db.get_issue(id)?.is_none() {
            bail!("Issue #{} not found", id);
        }
        Some(id)
    } else {
        None
    };

    // 3. Create worktree
    let (worktree_dir, branch_name) = create_worktree(&root, &slug, None)?;

    // Write slug sentinel so other commands can identify this worktree
    std::fs::write(worktree_dir.join(".kickoff-slug"), &slug)
        .context("Failed to write .kickoff-slug sentinel")?;

    // 4. Build prompt
    let prompt = build_plan_prompt(opts.doc, issue_id);

    // 5. Write PLAN_KICKOFF.md
    std::fs::write(worktree_dir.join("PLAN_KICKOFF.md"), &prompt)
        .context("Failed to write PLAN_KICKOFF.md")?;

    // 6. Exclude files from git
    exclude_kickoff_files(&worktree_dir)?;

    // Dry run: print and exit
    if opts.dry_run {
        let parent_id = AgentConfig::load(crosslink_dir)?
            .map(|c| c.agent_id)
            .unwrap_or_else(|| "driver".to_string());
        let agent_id = format!("{}--{}", parent_id, slug);
        println!("{}", prompt);
        println!("---");
        println!("Worktree: {}", worktree_dir.display());
        println!("Branch:   {}", branch_name);
        println!("Agent:    {}", agent_id);
        return Ok(());
    }

    // 7. Init worktree agent
    let agent_id = init_worktree_agent(&worktree_dir, crosslink_dir, &slug)?;

    // preflight is guaranteed Some after the dry-run early return above
    let preflight = preflight.context("preflight check was skipped unexpectedly")?;

    // 8. Launch with read-only tools
    let allowed_tools = build_allowed_tools_plan();
    let mut session_name = tmux_session_name(&slug);
    if tmux_session_exists(&session_name) {
        let suffix = rand_suffix();
        session_name = format!("{}-{}", &session_name[..session_name.len().min(44)], suffix);
    }

    // Plan mode reads PLAN_KICKOFF.md instead of KICKOFF.md
    let cmd = build_agent_command(
        preflight.timeout_cmd,
        opts.timeout.as_secs(),
        opts.model,
        &allowed_tools,
        "PLAN_KICKOFF.md",
        preflight.sandbox_command.as_deref(),
        &worktree_dir,
        false, // plan mode never skips permissions
    );

    launch_plan_in_tmux(&worktree_dir, &session_name, &cmd, crosslink_dir)?;

    // 9. Report
    if !opts.quiet {
        println!("Plan analysis agent launched (read-only mode).");
        println!();
        println!("  Worktree: {}", worktree_dir.display());
        println!("  Branch:   {}", branch_name);
        if let Some(id) = issue_id {
            println!("  Issue:    #{}", id);
        }
        println!("  Agent:    {}", agent_id);
        println!("  Session:  {}", session_name);
        println!();
        println!("  Approve trust:  tmux attach -t {}", session_name);
        println!("  Check status:   crosslink kickoff status {}", agent_id);
        println!("  View report:    crosslink kickoff show-plan {}", agent_id);
    } else {
        println!("{}", session_name);
    }

    Ok(())
}

/// Display a gap report from a previous plan analysis.
pub fn show_plan(crosslink_dir: &Path, agent: &str) -> Result<()> {
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

    let plan_file = worktree_dir.join(".kickoff-plan.json");
    if !plan_file.exists() {
        // Check status
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
            "No gap report found yet for '{}'. Agent status: {}",
            agent,
            status
        );
    }

    let content =
        std::fs::read_to_string(&plan_file).context("Failed to read .kickoff-plan.json")?;

    // Pretty-print the JSON
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
        println!(
            "{}",
            serde_json::to_string_pretty(&parsed).unwrap_or(content)
        );
    } else {
        // Not valid JSON — print raw
        print!("{}", content);
    }

    Ok(())
}
