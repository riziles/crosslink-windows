//! Agent request protocol (design doc §9).
//!
//! Cross-machine control of running agents happens via signed JSON
//! files on the `crosslink/hub` branch. A driver (operator) writes a
//! request under `agents/<target_id>/requests/<ulid>.json`; the target
//! agent polls its own `requests/` on every sync tick, validates the
//! signer, acts, and writes `agents/<target_id>/requests/<ulid>.ack.json`.
//!
//! This module defines the on-disk schema and path conventions. The
//! actual write (commit + push) is on [`crate::shared_writer::SharedWriter`];
//! the scan helpers here are for the read side (dashboard rendering).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Control actions a driver can request of an agent.
///
/// `kill` terminates the agent after the current tool use completes.
/// `pause` / `resume` write a pause flag the agent checks between
/// ticks. `reprioritise` nudges the agent toward a different issue
/// (subject-carried).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RequestKind {
    Kill,
    Pause,
    Resume,
    Reprioritise,
}

impl RequestKind {
    /// Parse a lowercase string from CLI/API surface.
    ///
    /// Named `parse` (not `from_str`) to sidestep the `std::str::FromStr`
    /// signature — we want `anyhow::Error` for rich context, not the
    /// trait's associated-error type.
    ///
    /// # Errors
    /// Returns an error if the input doesn't match one of the known kinds.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "kill" => Ok(Self::Kill),
            "pause" => Ok(Self::Pause),
            "resume" => Ok(Self::Resume),
            "reprioritise" | "reprioritize" => Ok(Self::Reprioritise),
            other => anyhow::bail!(
                "unknown request kind '{other}' (expected kill|pause|resume|reprioritise)"
            ),
        }
    }
}

/// Optional subject carried with some request kinds. `issue_id` is
/// the display id the operator saw on the panel; the agent resolves
/// it to a uuid at act time.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestSubject {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub issue_id: Option<i64>,
}

impl RequestSubject {
    pub const fn is_empty(&self) -> bool {
        self.issue_id.is_none()
    }
}

/// The on-disk request file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRequest {
    /// Lexicographically-sortable ulid; also the filename stem.
    pub request_id: String,
    pub kind: RequestKind,
    #[serde(default, skip_serializing_if = "RequestSubject::is_empty")]
    pub subject: RequestSubject,
    /// Driver fingerprint (SSH key signature). Matches `user.signingkey`
    /// on the workspace that issued the request.
    pub requested_by: String,
    pub requested_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Ack written by the target agent after handling a request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRequestAck {
    pub request_id: String,
    pub ack_at: String,
    /// `true` if the agent executed the requested action, `false` if it
    /// rejected (e.g., unknown signer, unsupported kind).
    pub acted: bool,
    /// Free-form summary of what happened (e.g., "killed", "paused",
    /// "ignored: already paused").
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// A request paired with its ack, if one has been written. Used by
/// the dashboard reader.
#[derive(Debug, Clone)]
pub struct RequestWithAck {
    pub request: AgentRequest,
    pub ack: Option<AgentRequestAck>,
}

/// `agents/<agent_id>/requests` relative to the hub-cache root.
pub fn requests_dir(agent_id: &str) -> PathBuf {
    PathBuf::from("agents").join(agent_id).join("requests")
}

/// Relative path to a single request file. Separate from the ack path
/// so callers don't accidentally collide.
pub fn request_path(agent_id: &str, request_id: &str) -> PathBuf {
    requests_dir(agent_id).join(format!("{request_id}.json"))
}

/// Scan an agent's request directory rooted at `cache_dir` and return
/// every request paired with its ack (if any). Missing directory is
/// treated as empty, not an error.
///
/// # Errors
/// Returns an error if a request file exists but is malformed JSON.
pub fn scan(cache_dir: &Path, agent_id: &str) -> Result<Vec<RequestWithAck>> {
    let dir = cache_dir.join(requests_dir(agent_id));
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir).with_context(|| format!("read_dir {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Skip acks in this pass; we'll pair them back in below.
        // The ack compound extension (`.ack.json`) has no clean Path
        // API, so lowercase the filename first for a case-insensitive
        // tail match. The plain `.json` check uses Path::extension so
        // clippy's case-sensitive-file-extension lint stays happy.
        if name.to_ascii_lowercase().ends_with(".ack.json") {
            continue;
        }
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            continue;
        }
        let raw =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let request: AgentRequest =
            serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;

        let ack_file = dir.join(format!("{}.ack.json", request.request_id));
        let ack = if ack_file.exists() {
            let ack_raw = std::fs::read_to_string(&ack_file)
                .with_context(|| format!("read {}", ack_file.display()))?;
            Some(
                serde_json::from_str::<AgentRequestAck>(&ack_raw)
                    .with_context(|| format!("parse {}", ack_file.display()))?,
            )
        } else {
            None
        };

        out.push(RequestWithAck { request, ack });
    }

    // Ulid file stems sort lex = chronological.
    out.sort_by(|a, b| a.request.request_id.cmp(&b.request.request_id));
    Ok(out)
}

/// Generate a fresh ulid for a request. Lexicographic sort = timestamp
/// sort within ~1ms resolution, which is all the request-ordering
/// protocol relies on.
pub fn new_request_id() -> String {
    ulid::Ulid::new().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_requestkind_roundtrip() {
        for (s, k) in [
            ("kill", RequestKind::Kill),
            ("pause", RequestKind::Pause),
            ("resume", RequestKind::Resume),
            ("reprioritise", RequestKind::Reprioritise),
            ("reprioritize", RequestKind::Reprioritise),
        ] {
            assert_eq!(RequestKind::parse(s).unwrap(), k);
        }
        assert!(RequestKind::parse("bogus").is_err());
    }

    #[test]
    fn test_scan_missing_dir_returns_empty() {
        let dir = tempdir().unwrap();
        let out = scan(dir.path(), "agent-x").unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn test_scan_pairs_requests_with_acks() {
        let dir = tempdir().unwrap();
        let req_dir = dir.path().join(requests_dir("agent-x"));
        std::fs::create_dir_all(&req_dir).unwrap();

        // Pending request (no ack).
        let r1 = AgentRequest {
            request_id: "01HXY000000000000000000001".into(),
            kind: RequestKind::Pause,
            subject: RequestSubject { issue_id: Some(42) },
            requested_by: "SHA256:driver".into(),
            requested_at: "2026-04-20T18:30:00Z".into(),
            reason: Some("stuck".into()),
        };
        std::fs::write(
            req_dir.join(format!("{}.json", r1.request_id)),
            serde_json::to_string(&r1).unwrap(),
        )
        .unwrap();

        // Acked request.
        let r2 = AgentRequest {
            request_id: "01HXY000000000000000000000".into(),
            kind: RequestKind::Kill,
            subject: RequestSubject::default(),
            requested_by: "SHA256:driver".into(),
            requested_at: "2026-04-20T18:20:00Z".into(),
            reason: None,
        };
        std::fs::write(
            req_dir.join(format!("{}.json", r2.request_id)),
            serde_json::to_string(&r2).unwrap(),
        )
        .unwrap();
        let ack = AgentRequestAck {
            request_id: r2.request_id.clone(),
            ack_at: "2026-04-20T18:20:05Z".into(),
            acted: true,
            result: "killed".into(),
            notes: None,
        };
        std::fs::write(
            req_dir.join(format!("{}.ack.json", r2.request_id)),
            serde_json::to_string(&ack).unwrap(),
        )
        .unwrap();

        let out = scan(dir.path(), "agent-x").unwrap();
        assert_eq!(out.len(), 2);
        // Sorted lex; r2's id is lower so it comes first.
        assert_eq!(out[0].request.request_id, r2.request_id);
        assert!(out[0].ack.as_ref().unwrap().acted);
        assert_eq!(out[1].request.request_id, r1.request_id);
        assert!(out[1].ack.is_none());
    }

    #[test]
    fn test_new_request_id_is_unique_and_sortable() {
        let a = new_request_id();
        let b = new_request_id();
        assert_ne!(a, b);
        // Ulids are 26 chars uppercase.
        assert_eq!(a.len(), 26);
        assert_eq!(b.len(), 26);
    }
}
