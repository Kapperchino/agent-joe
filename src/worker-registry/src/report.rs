use super::{
    WorkerExecution,
    budget::{BudgetState, BudgetUsage},
    request::{WorkerRequest, WorkerRole},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tools::tool_defs::{ToolEffect, ToolResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Registered,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    BudgetExhausted,
    Interrupted,
}

impl WorkerStatus {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Failed
                | Self::Cancelled
                | Self::TimedOut
                | Self::BudgetExhausted
                | Self::Interrupted
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerReport {
    pub worker_id: String,
    pub status: WorkerStatus,
    pub findings: String,
    pub changed_files: Vec<String>,
    pub possibly_changed_files: Vec<String>,
    pub validation: Vec<ToolResult>,
    #[serde(default)]
    pub edits: Vec<utils::changes::EditSummary>,
    #[serde(default)]
    pub processes: Vec<utils::cargo::CargoResult>,
    pub unresolved_issues: Vec<String>,
    pub artifacts: Vec<utils::artifacts::ArtifactReference>,
    pub budget: BudgetUsage,
    pub duration_ms: u128,
    pub completion_criteria: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerView {
    pub worker_id: String,
    pub request: WorkerRequest,
    pub status: WorkerStatus,
    pub report: Option<WorkerReport>,
}

impl WorkerView {
    pub fn recover(&mut self) {
        let report = self.recovered_report();
        self.status = report.status;
        self.report = Some(report);
    }

    pub(super) fn recovered_report(&self) -> WorkerReport {
        match self.report.as_ref()
            .filter(|_| self.status.terminal())
            .filter(|report| report.status == self.status)
            .filter(|report| report.worker_id == self.worker_id) {
            Some(report) => report.clone(),
            _ => WorkerReport {
                worker_id: self.worker_id.clone(),
                status: WorkerStatus::Interrupted,
                findings: "Worker did not commit a final report before restart".into(),
                changed_files: Vec::new(),
                possibly_changed_files: Vec::new(),
                validation: Vec::new(),
                edits: Vec::new(),
                processes: Vec::new(),
                unresolved_issues: vec!["Effects and requested checks are uncertain; inspect the workspace and saved descendant tool artifacts before retrying. Saved workers are never restarted automatically.".into()],
                artifacts: Vec::new(),
                budget: BudgetUsage::default(),
                duration_ms: 0,
                completion_criteria: self.request.completion_criteria.clone(),
            },
        }
    }
}

#[derive(Default)]
pub(super) struct Evidence {
    pub changed_files: BTreeSet<String>,
    pub possibly_changed_files: BTreeSet<String>,
    pub validation: Vec<ToolResult>,
    pub edits: Vec<utils::changes::EditSummary>,
    pub unresolved: Vec<String>,
}

impl Evidence {
    pub fn record(&mut self, effect: ToolEffect, result: &ToolResult) {
        let edit = result
            .outcome
            .as_ref()
            .ok()
            .filter(|_| {
                matches!(
                    result.invocation.name.as_ref(),
                    "apply_patch" | "undo_changes"
                )
            })
            .and_then(|content| serde_json::from_str::<serde_json::Value>(content).ok())
            .map(|output| output.get("edit").cloned().unwrap_or(output))
            .and_then(|output| serde_json::from_value::<utils::changes::EditSummary>(output).ok());
        if let Some(edit) = edit {
            self.changed_files
                .extend(edit.applied.iter().map(|path| path.display().to_string()));
            self.possibly_changed_files
                .extend(edit.in_flight.iter().map(|path| path.display().to_string()));
            self.edits.push(edit);
        }
        match (effect, result.invocation.name.as_ref()) {
            (ToolEffect::Validate | ToolEffect::ProcessControl, _) | (_, "cargo") => {
                self.validation.push(result.clone())
            }
            _ => {}
        }
        if let (ToolEffect::Write, "cargo" | "worktree") = (effect, result.invocation.name.as_ref())
        {
            self.unresolved.push(format!(
                "{} may modify workspace files; its changed paths are not enumerated in this report and require review",
                result.invocation.name
            ));
        }
        if let Err(failure) = &result.outcome {
            self.unresolved
                .push(format!("{}: {failure}", result.invocation.name));
        }
        let patches = match effect {
            ToolEffect::Write => result
                .invocation
                .input
                .get("patch")
                .and_then(serde_json::Value::as_str)
                .and_then(|patch| utils::diff::DiffSet::new(patch).ok()),
            _ => None,
        };
        if let Some(patches) = patches {
            use utils::diff::Patch;
            let paths = patches.patches().iter().flat_map(|patch| match patch {
                Patch::AddFile { path, .. }
                | Patch::DeleteFile { path }
                | Patch::UpdateFile { path, .. } => vec![path.display().to_string()],
                Patch::MoveFile { from, to, .. } => {
                    vec![from.display().to_string(), to.display().to_string()]
                }
            });
            match &result.outcome {
                Ok(_) => self.changed_files.extend(paths),
                Err(failure)
                    if failure.effects == tools::tool_error::ToolEffects::MayHaveChanged =>
                {
                    self.possibly_changed_files.extend(paths)
                }
                _ => {}
            }
        }
    }
}

#[derive(Default)]
pub struct StoredWorkerEvidence {
    pub artifacts: Vec<utils::artifacts::ArtifactReference>,
    pub processes: Vec<utils::cargo::CargoResult>,
    pub unresolved_issues: Vec<String>,
}

pub enum WorkerOutcome {
    Completed(String),
    Failed(String),
    Cancelled,
    TimedOut,
}

impl WorkerOutcome {
    fn status(&self) -> WorkerStatus {
        match self {
            Self::Completed(_) => WorkerStatus::Completed,
            Self::Failed(_) => WorkerStatus::Failed,
            Self::Cancelled => WorkerStatus::Cancelled,
            Self::TimedOut => WorkerStatus::TimedOut,
        }
    }

    fn findings(self) -> String {
        match self {
            Self::Completed(findings) | Self::Failed(findings) => findings,
            Self::Cancelled => "Worker cancelled after cleanup".into(),
            Self::TimedOut => {
                "Worker deadline exceeded; owned work was cancelled and cleaned up".into()
            }
        }
    }
}

impl WorkerExecution {
    pub fn report(
        &self,
        outcome: WorkerOutcome,
        elapsed: std::time::Duration,
        stored: StoredWorkerEvidence,
    ) -> WorkerReport {
        let evidence = self.evidence.lock().unwrap();
        let budget = self.budget.usage();
        let status = match budget.state {
            BudgetState::Available => outcome.status(),
            BudgetState::Exhausted => WorkerStatus::BudgetExhausted,
        };
        let mut unresolved = evidence.unresolved.clone();
        unresolved.extend(stored.unresolved_issues);
        let findings = outcome.findings();
        let findings = match findings.len() <= 8192 {
            true => findings,
            false => {
                unresolved.push("Model explanation was truncated to 8 KiB; the full response remains in the worker session".into());
                findings[..findings.floor_char_boundary(8192)].to_owned()
            }
        };
        if let (WorkerRole::Write, []) = (self.request.role, evidence.validation.as_slice()) {
            unresolved.push("No validation checks were executed by this worker; the parent must assess the completion criteria and validate changes".into());
        }
        WorkerReport {
            worker_id: self.id.clone(),
            status,
            findings,
            changed_files: evidence.changed_files.iter().cloned().collect(),
            possibly_changed_files: evidence.possibly_changed_files.iter().cloned().collect(),
            validation: evidence.validation.clone(),
            edits: evidence.edits.clone(),
            processes: stored.processes,
            unresolved_issues: unresolved,
            artifacts: stored.artifacts,
            budget,
            duration_ms: elapsed.as_millis(),
            completion_criteria: self.request.completion_criteria.clone(),
        }
    }
}
