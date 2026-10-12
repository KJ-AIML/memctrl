//! Read-only Heli history import with reviewable proposals and atomic lineage.
use crate::{
    model::{ClaimType, LifecycleState, NewMemory, RelationType, VerificationState},
    sanitize::{sanitize_revision, sanitize_text, sanitize_value, validate_identifier},
    store::MemoryStore,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeliTaskRecord {
    pub task_id: String,
    pub title: String,
    pub status: String,
    pub target_repo: String,
    pub risk_tier: String,
    pub completed_at: Option<String>,
    pub commit_sha: Option<String>,
    pub plan_steps: Vec<Value>,
    pub evidence_items: Vec<String>,
    pub decisions: Vec<String>,
    pub diagnosis: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LessonProposal {
    pub proposal_id: String,
    pub claim: String,
    pub claim_type: ClaimType,
    pub source_tasks: Vec<String>,
    pub source_revisions: Vec<String>,
    pub supporting_evidence: Vec<String>,
    pub contradictory_evidence: Vec<String>,
    pub applicability_conditions: Vec<String>,
    pub counterexamples: Vec<String>,
    pub proposed_disposition: LifecycleState,
}

fn read_json(path: &Path) -> Result<Option<Value>> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(path).context("Unable to read Heli JSON")?;
    // Sanitizing raw JSON first can corrupt escapes and hide sensitive nested keys.
    let value: Value = serde_json::from_str(&text).context("Invalid Heli JSON")?;
    Ok(Some(sanitize_value(&value)))
}
fn text_file(path: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(sanitize_text(
        &fs::read_to_string(path).context("Unable to read Heli text")?,
    )))
}
fn string(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or("").into()
}
fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| item.to_string())
            })
            .collect(),
        Some(Value::String(text)) if !text.is_empty() => vec![text.clone()],
        _ => vec![],
    }
}

fn collect_task_dirs(path: &Path, output: &mut Vec<PathBuf>, depth: usize) -> Result<()> {
    ensure!(depth < 32, "Heli task nesting exceeds safety limit");
    for entry in fs::read_dir(path).context("Unable to inspect Heli history")? {
        let entry = entry?;
        let kind = entry.file_type()?;
        // Do not follow links into arbitrary directories or cyclic trees.
        if !kind.is_dir() || kind.is_symlink() {
            continue;
        }
        let dir = entry.path();
        if dir.join("task.json").is_file()
            || dir.join("current-task.md").is_file()
            || dir.join("decisions.md").is_file()
        {
            output.push(dir);
        } else {
            collect_task_dirs(&dir, output, depth + 1)?;
        }
    }
    Ok(())
}

/// Load completed nested task records without writing or locking the source workspace.
pub fn load_tasks(workspace: &Path) -> Result<Vec<HeliTaskRecord>> {
    let harness = if workspace.join(".heli-harness").is_dir() {
        workspace.join(".heli-harness")
    } else if workspace.join(".heli").is_dir() {
        workspace.join(".heli")
    } else if workspace
        .file_name()
        .is_some_and(|n| n == ".heli-harness" || n == ".heli")
    {
        workspace.to_path_buf()
    } else {
        return Ok(vec![]);
    };
    let tasks = harness.join("tasks");
    if !tasks.is_dir() {
        return Ok(vec![]);
    }
    let mut dirs = Vec::new();
    collect_task_dirs(&tasks, &mut dirs, 0)?;
    dirs.sort_by(|a, b| {
        let modified = |p: &PathBuf| fs::metadata(p).ok().and_then(|m| m.modified().ok());
        modified(b).cmp(&modified(a)).then_with(|| a.cmp(b))
    });
    let mut records = Vec::new();
    for dir in dirs {
        let raw = if dir.join("task.json").is_file() {
            // Revision identity is the sole narrowly scoped exception to secret heuristics.
            let value: Value = serde_json::from_str(&fs::read_to_string(dir.join("task.json"))?)
                .context("Invalid Heli task JSON")?;
            let revision = value
                .get("commitSha")
                .and_then(Value::as_str)
                .map(sanitize_revision);
            (sanitize_value(&value), revision)
        } else {
            (json!({}), None)
        };
        let data = raw.0;
        let task_id = dir
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid Heli task directory"))?
            .to_string();
        validate_identifier(&task_id, "Heli task ID")?;
        let mut record = HeliTaskRecord {
            task_id: task_id.clone(),
            title: string(&data, "title"),
            status: string(&data, "status"),
            target_repo: data
                .pointer("/target/repositoryId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| data.get("repo").and_then(Value::as_str).unwrap_or(""))
                .into(),
            risk_tier: string(&data, "riskTier"),
            completed_at: data
                .get("completedAt")
                .and_then(Value::as_str)
                .map(str::to_owned),
            commit_sha: raw.1,
            plan_steps: vec![],
            evidence_items: strings(data.get("evidence_items").or_else(|| data.get("evidence"))),
            decisions: strings(data.get("decisions")),
            diagnosis: read_json(&dir.join("diagnosis.json"))?,
        };
        if record.title.is_empty() {
            record.title = task_id;
        }
        if record.status.is_empty() {
            record.status = "unknown".into();
        }
        if record.risk_tier.is_empty() {
            record.risk_tier = "S1".into();
        }
        if let Some(current) = text_file(&dir.join("current-task.md"))? {
            for line in current.lines() {
                let line = line.trim();
                if let Some(value) = line.strip_prefix("Current status:") {
                    record.status = value.trim().into();
                }
                if let Some(value) = line.strip_prefix("Target repo:") {
                    if !value.trim().is_empty() {
                        record.target_repo = value.trim().into();
                    }
                }
                if let Some(value) = line.strip_prefix("Risk tier:") {
                    record.risk_tier = value.trim().into();
                }
            }
        }
        if let Some(plan) = text_file(&dir.join("plan.md"))? {
            let mut step: Option<Value> = None;
            for line in plan.lines() {
                let line = line.trim();
                if let Some(header) = line.strip_prefix("## Step ") {
                    if let Some(previous) = step.take() {
                        record.plan_steps.push(previous);
                    }
                    step = Some(json!({"name":header,"status":"pending","evidence":""}));
                } else if let Some(current) = step.as_mut() {
                    if let Some(status) = line.strip_prefix("Status:") {
                        current["status"] = json!(status.trim());
                    }
                    if let Some(evidence) = line.strip_prefix("Evidence:") {
                        current["evidence"] = json!(evidence.trim());
                        if !evidence.trim().is_empty() {
                            record.evidence_items.push(evidence.trim().into());
                        }
                    }
                }
            }
            if let Some(last) = step {
                record.plan_steps.push(last);
            }
        }
        if let Some(decisions) = text_file(&dir.join("decisions.md"))? {
            for line in decisions.lines() {
                if let Some(decision) = line
                    .trim()
                    .strip_prefix("- ")
                    .or_else(|| line.trim().strip_prefix("* "))
                {
                    record.decisions.push(decision.into());
                }
            }
        }
        for filename in ["evidence.json", "evidence.md"] {
            let path = dir.join(filename);
            if filename.ends_with(".json") {
                if let Some(value) = read_json(&path)? {
                    record.evidence_items.extend(strings(
                        value
                            .get("items")
                            .or_else(|| value.get("evidence"))
                            .or(Some(&value)),
                    ));
                }
            } else if let Some(text) = text_file(&path)? {
                record.evidence_items.extend(
                    text.lines()
                        .filter_map(|l| l.trim().strip_prefix("- ").map(str::to_owned)),
                );
            }
        }
        if matches!(
            record.status.to_lowercase().as_str(),
            "complete" | "completed" | "done"
        ) {
            records.push(record);
        }
    }
    Ok(records)
}

/// Deterministic candidate proposals, including refuted history and divergent counterexamples.
pub fn distill(tasks: &[HeliTaskRecord]) -> Vec<LessonProposal> {
    let mut proposals = Vec::new();
    for task in tasks {
        let base = |id: String,
                    claim: String,
                    evidence: Vec<String>,
                    contradictions: Vec<String>,
                    disposition| LessonProposal {
            proposal_id: id,
            claim,
            claim_type: ClaimType::DerivedLesson,
            source_tasks: vec![task.task_id.clone()],
            source_revisions: task.commit_sha.iter().cloned().collect(),
            supporting_evidence: evidence,
            contradictory_evidence: contradictions,
            applicability_conditions: vec![format!("Repo context: {}", task.target_repo)],
            counterexamples: vec![],
            proposed_disposition: disposition,
        };
        if let Some(diagnosis) = &task.diagnosis {
            let hyp = string(diagnosis, "active_hypothesis");
            let boundary = string(diagnosis, "closest_proven_boundary");
            if !hyp.is_empty() && !boundary.is_empty() {
                proposals.push(base(format!("prop-{}-diag",task.task_id), format!("When encountering {boundary}, verify hypothesis '{hyp}' before costly retries."),
                    vec![format!("Closest proven boundary: {boundary}")], strings(diagnosis.get("contradicted_hypotheses")).into_iter().map(|c| format!("Contradicted hypothesis: {c}")).collect(),LifecycleState::Candidate));
            }
        }
        for (i, decision) in task.decisions.iter().enumerate() {
            if decision.chars().count() <= 15 {
                continue;
            }
            let refuted = decision.to_lowercase().contains("refuted");
            proposals.push(base(
                format!("prop-{}-dec-{i}", task.task_id),
                decision.clone(),
                task.evidence_items.iter().take(2).cloned().collect(),
                vec![],
                if refuted {
                    LifecycleState::Rejected
                } else {
                    LifecycleState::Candidate
                },
            ));
        }
    }
    let words = |text: &str| -> BTreeSet<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() >= 4)
            .map(str::to_lowercase)
            .collect()
    };
    for i in 0..proposals.len() {
        for j in i + 1..proposals.len() {
            if proposals[i].source_tasks == proposals[j].source_tasks {
                continue;
            }
            if words(&proposals[i].claim)
                .intersection(&words(&proposals[j].claim))
                .count()
                >= 3
            {
                let a = format!(
                    "Task {} addressed a related symptom with a different resolution: '{}'",
                    proposals[j].source_tasks[0],
                    proposals[j].claim.chars().take(80).collect::<String>()
                );
                let b = format!(
                    "Task {} addressed a related symptom with a different resolution: '{}'",
                    proposals[i].source_tasks[0],
                    proposals[i].claim.chars().take(80).collect::<String>()
                );
                proposals[i].counterexamples.push(a);
                proposals[j].counterexamples.push(b);
            }
        }
    }
    proposals
}

/// All memories and external evidence in this batch commit or roll back together.
pub fn persist_proposals(
    store: &mut MemoryStore,
    proposals: &[LessonProposal],
) -> Result<Vec<String>> {
    persist_proposals_with(store, proposals, &crate::guard::Policy::default())
}

/// `persist_proposals` with an explicit write-guard policy (from `.memoryrc`).
pub fn persist_proposals_with(
    store: &mut MemoryStore,
    proposals: &[LessonProposal],
    policy: &crate::guard::Policy,
) -> Result<Vec<String>> {
    store.conn.execute_batch("SAVEPOINT heli_review")?;
    let result: Result<Vec<String>> = (|| {
        let mut ids = Vec::new();
        for proposal in proposals {
            validate_identifier(&proposal.proposal_id, "Heli proposal ID")?;
            ensure!(
                !proposal.source_tasks.is_empty(),
                "Heli proposal requires source lineage"
            );
            ensure!(
                matches!(
                    proposal.proposed_disposition,
                    LifecycleState::Candidate | LifecycleState::Rejected
                ),
                "Heli proposals must remain reviewable candidates or rejected history"
            );
            ensure!(
                proposal.claim_type == ClaimType::DerivedLesson,
                "Heli proposals require derived lesson claim type"
            );
            let refuted = proposal.claim.to_lowercase().contains("refuted");
            let mut content = proposal.claim.clone();
            if !proposal.counterexamples.is_empty() {
                content.push_str(&format!(
                    "\nCounterexamples:\n- {}",
                    proposal.counterexamples.join("\n- ")
                ));
            }
            let mut lesson = NewMemory {
                layer: "project".into(),
                content: sanitize_text(&content),
                source: "heli-distillation".into(),
                confidence: 0.7,
                tags: vec!["distilled".into(), "heli-history".into()],
                claim_type: ClaimType::DerivedLesson,
                lifecycle_state: if refuted {
                    LifecycleState::Rejected
                } else {
                    proposal.proposed_disposition
                },
                verification_state: if refuted {
                    VerificationState::Refuted
                } else {
                    VerificationState::Unverified
                },
                ..NewMemory::default()
            };
            // Task history can carry injected text; flagged lessons are tagged for the reviewer.
            crate::guard::apply(&mut lesson, policy);
            let id = store.insert_memory(lesson)?;
            for (index, task) in proposal.source_tasks.iter().enumerate() {
                validate_identifier(task, "Heli task ID")?;
                let revision = proposal
                    .source_revisions
                    .get(index)
                    .or_else(|| proposal.source_revisions.first())
                    .map(|r| sanitize_revision(r));
                store.add_evidence(&id,"heli",task,revision.as_deref(),RelationType::DerivedFrom, sanitize_value(&json!({
                    "proposal_id":proposal.proposal_id,"applicability":proposal.applicability_conditions,
                    "supporting_evidence":proposal.supporting_evidence,"contradictory_evidence":proposal.contradictory_evidence,"counterexamples":proposal.counterexamples,
                })))?;
            }
            ids.push(id);
        }
        Ok(ids)
    })();
    match result {
        Ok(ids) => {
            store.conn.execute_batch("RELEASE heli_review")?;
            Ok(ids)
        }
        Err(error) => {
            store
                .conn
                .execute_batch("ROLLBACK TO heli_review; RELEASE heli_review")?;
            Err(error)
        }
    }
}

pub fn review(store: &mut MemoryStore, path: &Path, persist: bool) -> Result<Value> {
    review_filtered(store, path, persist, 20, None)
}

pub fn review_filtered(
    store: &mut MemoryStore,
    path: &Path,
    persist: bool,
    limit: usize,
    repo_filter: Option<&str>,
) -> Result<Value> {
    review_with_policy(
        store,
        path,
        persist,
        limit,
        repo_filter,
        &crate::guard::Policy::default(),
    )
}

/// `review_filtered` with an explicit write-guard policy (from `.memoryrc`).
pub fn review_with_policy(
    store: &mut MemoryStore,
    path: &Path,
    persist: bool,
    limit: usize,
    repo_filter: Option<&str>,
    policy: &crate::guard::Policy,
) -> Result<Value> {
    let tasks: Vec<_> = load_tasks(path)?
        .into_iter()
        .filter(|task| repo_filter.is_none_or(|repo| task.target_repo.eq_ignore_ascii_case(repo)))
        .take(limit)
        .collect();
    let proposals = distill(&tasks);
    let ids = if persist {
        persist_proposals_with(store, &proposals, policy)?
    } else {
        vec![]
    };
    // Struct fields are sanitized on ingestion; preserve narrowly validated revision identities.
    Ok(
        json!({"tasks_loaded":tasks.len(),"tasks":tasks,"proposals":proposals,"persisted":persist,"created_ids":ids}),
    )
}
