//! Jev-driven navigation (0004, 0041): for airline sites, miles
//! programs, and recovery when a generated Google Flights URL stops
//! producing the expected page.
//!
//! Ported from vs1-browser's run loop (`main.rs`, `run_agent_page`) and
//! its "questions" policy. Each step observes the page, asks Jev for an
//! operation and a target among the observed controls, and performs it
//! through the snapshot's input guards. Targets are code-owned node
//! references: nothing Jev answers becomes a selector, coordinate or
//! script.
//!
//! Unlike vs1, no text model writes field values. A [`Task`] carries
//! the values it may type (`{"origin": "São Paulo", …}`), and when Jev
//! chooses to type into a field, a second Choice question picks which
//! known value fits it, or none.

mod policy;

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tm_jev::{Jev, Usage};

use crate::{
    error::{Error, Result},
    page::Page,
    snapshot::{Action, ActionKind, Snapshot},
};

/// Consecutive actions without visible effect that end a run as
/// blocked.
const INEFFECTIVE_LIMIT: usize = 6;

/// What the agent should achieve on the current page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// The goal in natural language.
    pub goal: String,
    /// Values the agent may type, by name: `{"origin": "São Paulo",
    /// "departure date": "2026-11-10"}`. The name `none` is reserved.
    pub values: BTreeMap<String, String>,
    /// The most actions to perform. Decisions (including retries after
    /// the page changed under a decision) are limited to twice this.
    pub max_steps: usize,
    /// Independent checks of a DONE: the final URL must contain this.
    pub expect_url: Option<String>,
    /// Independent checks of a DONE: the final visible text must
    /// contain each of these.
    pub expect_text: Vec<String>,
}

impl Task {
    /// A goal with no values, 60 steps and no checks.
    pub fn new(goal: impl Into<String>) -> Self {
        Self {
            goal: goal.into(),
            values: BTreeMap::new(),
            max_steps: 60,
            expect_url: None,
            expect_text: Vec::new(),
        }
    }

    pub fn value(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.values.insert(name.into(), value.into());
        self
    }
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Jev said every requirement is visible. A claim; see
    /// [`AgentOutcome::verification`].
    Done,
    /// Jev said nothing can progress, or six actions in a row had no
    /// visible effect.
    Blocked,
    /// The step or decision budget ran out.
    Exhausted,
    /// The browser or Jev failed; `reason` says how.
    Failed,
}

/// The result of [`run`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentOutcome {
    pub status: Status,
    pub reason: Option<String>,
    /// The task's checks, run on a fresh observation after the run.
    /// `None` when the task has no checks.
    pub verification: Option<Verification>,
    pub trace: Trace,
}

impl AgentOutcome {
    /// Done, and every check (if any) passed.
    pub fn succeeded(&self) -> bool {
        self.status == Status::Done
            && self.verification.as_ref().is_none_or(|v| v.passed)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verification {
    pub passed: bool,
    pub checks: BTreeMap<String, bool>,
}

/// Everything a run did. May contain page text and typed values.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub goal: String,
    pub steps: Vec<Step>,
    pub decisions: Vec<DecisionRecord>,
    /// Times the page changed between a decision and its input, so the
    /// decision was dropped and the page observed again.
    pub stale_retries: usize,
    pub jev_usage: Usage,
    pub final_url: Option<String>,
    pub elapsed_ms: u64,
}

/// One action performed, or (for a field no known value fits) skipped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub number: usize,
    pub operation: String,
    pub action: Action,
    pub target: Option<String>,
    /// The known value's name, for TYPE_TEXT.
    pub value_key: Option<String>,
    pub text: Option<String>,
    pub url: String,
    pub before_fingerprint: String,
    /// Whether the page's meaning changed after the action. `None` if
    /// it was skipped or the observation failed.
    pub page_changed: Option<bool>,
    /// Why no input was sent.
    pub skipped: Option<String>,
    pub elapsed_ms: u64,
}

/// One Jev call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRecord {
    /// `operation` or `value`.
    pub question: String,
    /// The operation (or value name) chosen.
    pub choice: Option<String>,
    pub target: Option<String>,
    pub confidence: Option<f64>,
    pub model: Option<String>,
    pub latency_ms: u64,
    pub error: Option<String>,
}

/// Drives `page` toward `task.goal`, one Jev decision per step.
pub async fn run(page: &Page, jev: &dyn Jev, task: &Task) -> AgentOutcome {
    let started = Instant::now();
    let mut runner = Runner {
        page,
        jev,
        task,
        started,
        trace: Trace {
            goal: task.goal.clone(),
            ..Trace::default()
        },
    };
    let (status, reason) = runner.run().await;
    let mut trace = runner.trace;
    let final_snapshot = page.snapshot().await.ok();
    trace.final_url = final_snapshot.as_ref().map(|s| s.url.clone());
    trace.elapsed_ms = millis(started.elapsed());
    let verification = verify(task, final_snapshot.as_ref());
    AgentOutcome {
        status,
        reason,
        verification,
        trace,
    }
}

fn verify(task: &Task, snapshot: Option<&Snapshot>) -> Option<Verification> {
    if task.expect_url.is_none() && task.expect_text.is_empty() {
        return None;
    }
    let (url, text) =
        snapshot.map_or(("", ""), |s| (s.url.as_str(), s.text.as_str()));
    let mut checks = BTreeMap::new();
    if let Some(expected) = &task.expect_url {
        checks.insert(
            format!("url contains {expected}"),
            url.contains(expected.as_str()),
        );
    }
    for expected in &task.expect_text {
        checks.insert(
            format!("text contains {expected}"),
            text.contains(expected.as_str()),
        );
    }
    Some(Verification {
        passed: snapshot.is_some() && checks.values().all(|passed| *passed),
        checks,
    })
}

struct Runner<'a> {
    page: &'a Page,
    jev: &'a dyn Jev,
    task: &'a Task,
    started: Instant,
    trace: Trace,
}

type Ending = (Status, Option<String>);

impl Runner<'_> {
    async fn run(&mut self) -> Ending {
        if self.task.values.contains_key(policy::NONE) {
            return (
                Status::Failed,
                Some(format!(
                    "`{}` is reserved and cannot name a value",
                    policy::NONE
                )),
            );
        }
        let mut snapshot = match self.page.snapshot().await {
            Ok(snapshot) => snapshot,
            Err(error) => return (Status::Failed, Some(error.to_string())),
        };
        // As in vs1: at most twice as many decision cycles as actions,
        // counting the cycles a stale page cut short.
        for _ in 0..self.task.max_steps.saturating_mul(2) {
            match self.step(&mut snapshot).await {
                Ok(Some(ending)) => return ending,
                Ok(None) => {}
                Err(error) if error.is_stale() => {
                    self.trace.stale_retries += 1;
                    match self.page.snapshot().await {
                        Ok(fresh) => snapshot = fresh,
                        Err(error) => {
                            return (Status::Failed, Some(error.to_string()));
                        }
                    }
                }
                Err(error) => return (Status::Failed, Some(error.to_string())),
            }
        }
        (Status::Exhausted, Some("decision budget exhausted".into()))
    }

    async fn step(
        &mut self,
        snapshot: &mut Snapshot,
    ) -> Result<Option<Ending>> {
        if !self.page.is_fresh(snapshot, None).await? {
            *snapshot = self.page.snapshot().await?;
        }
        let prepared = policy::request(
            snapshot,
            &self.task.goal,
            &self.trace.steps,
            &self.task.values,
        );
        let asked = Instant::now();
        let answer = self.jev.ask(&prepared.request).await;
        let decision = answer
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|response| policy::resolve(&prepared, response));
        self.record(policy::OPERATION, asked.elapsed(), &answer, |record| {
            if let Ok(decision) = &decision {
                record.choice = Some(decision.operation.clone());
                record.target = decision.target.clone();
                record.confidence = Some(decision.confidence);
            }
        });
        let decision = decision?;
        if decision.operation == policy::DONE
            || decision.operation == policy::BLOCKED
        {
            if !self.page.is_fresh(snapshot, None).await? {
                return Err(Error::Stale(
                    "page changed before the terminal decision".into(),
                ));
            }
            let status = if decision.operation == policy::DONE {
                Status::Done
            } else {
                Status::Blocked
            };
            return Ok(Some((status, None)));
        }
        let performed = self
            .trace
            .steps
            .iter()
            .filter(|s| s.skipped.is_none())
            .count();
        if performed >= self.task.max_steps {
            return Ok(Some((
                Status::Exhausted,
                Some("step budget exhausted".into()),
            )));
        }
        let Some(action) = decision.action else {
            return Err(Error::Jev(tm_jev::JevError::Malformed(format!(
                "operation {} has no action",
                decision.operation
            ))));
        };
        let mut text = None;
        let mut value_key = None;
        if action.kind == ActionKind::Fill {
            if !self.page.is_fresh(snapshot, None).await? {
                return Err(Error::Stale(
                    "page changed before choosing a value".into(),
                ));
            }
            match self.choose_value(&action, snapshot).await? {
                Some(key) => {
                    text = self.task.values.get(&key).cloned();
                    value_key = Some(key);
                }
                None => {
                    self.push_step(
                        &decision.operation,
                        decision.target,
                        action,
                        snapshot,
                        None,
                        None,
                    );
                    if let Some(step) = self.trace.steps.last_mut() {
                        step.skipped =
                            Some("no known value fits this field".into());
                    }
                    return Ok(None);
                }
            }
        }
        self.page.act(snapshot, &action, text.as_deref()).await?;
        tracing::debug!(
            operation = %decision.operation,
            label = %action.label,
            "agent step"
        );
        self.push_step(
            &decision.operation,
            decision.target,
            action.clone(),
            snapshot,
            value_key,
            text,
        );
        let before = snapshot.clone();
        *snapshot = self.page.snapshot_after(&before, &action).await?;
        let changed = snapshot.fingerprint != before.fingerprint;
        if let Some(step) = self.trace.steps.last_mut() {
            step.page_changed = Some(changed);
        }
        let steps = &self.trace.steps;
        if steps.len() >= INEFFECTIVE_LIMIT
            && steps.iter().rev().take(INEFFECTIVE_LIMIT).all(|s| {
                s.page_changed == Some(false)
                    && s.action.kind != ActionKind::Wait
            })
        {
            return Ok(Some((
                Status::Blocked,
                Some(format!(
                    "{INEFFECTIVE_LIMIT} actions in a row had no visible effect"
                )),
            )));
        }
        Ok(None)
    }

    /// Asks which known value belongs in the field; `None` for none.
    async fn choose_value(
        &mut self,
        action: &Action,
        snapshot: &Snapshot,
    ) -> Result<Option<String>> {
        let request = policy::value_request(
            &self.task.goal,
            action,
            snapshot,
            &self.trace.steps,
            &self.task.values,
        );
        let asked = Instant::now();
        let answer = self.jev.ask(&request).await;
        let choice =
            answer.as_ref().map_err(Clone::clone).and_then(|response| {
                policy::validate_choice(
                    response,
                    policy::VALUE_QUESTION,
                    &request,
                )
            });
        self.record(
            policy::VALUE_QUESTION,
            asked.elapsed(),
            &answer,
            |record| {
                if let Ok((choice, confidence)) = &choice {
                    record.choice = Some(choice.clone());
                    record.confidence = Some(*confidence);
                }
            },
        );
        let (choice, _) = choice?;
        Ok((choice != policy::NONE).then_some(choice))
    }

    fn record(
        &mut self,
        question: &str,
        latency: Duration,
        answer: &Result<tm_jev::Response, tm_jev::JevError>,
        fill: impl FnOnce(&mut DecisionRecord),
    ) {
        let mut record = DecisionRecord {
            question: question.to_owned(),
            choice: None,
            target: None,
            confidence: None,
            model: answer.as_ref().ok().map(|r| r.model.clone()),
            latency_ms: millis(latency),
            error: None,
        };
        if let Ok(response) = answer {
            self.trace.jev_usage += &response.usage();
        }
        fill(&mut record);
        if record.choice.is_none() {
            record.error = Some(match answer {
                Err(error) => error.to_string(),
                Ok(_) => "the answer did not match the question".into(),
            });
        }
        self.trace.decisions.push(record);
    }

    fn push_step(
        &mut self,
        operation: &str,
        target: Option<String>,
        action: Action,
        snapshot: &Snapshot,
        value_key: Option<String>,
        text: Option<String>,
    ) {
        self.trace.steps.push(Step {
            number: self.trace.steps.len() + 1,
            operation: operation.to_owned(),
            action,
            target,
            value_key,
            text,
            url: snapshot.url.clone(),
            before_fingerprint: snapshot.fingerprint.clone(),
            page_changed: None,
            skipped: None,
            elapsed_ms: millis(self.started.elapsed()),
        });
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_need_every_expectation_and_an_observation() {
        let mut task = Task::new("open Casa Flora");
        assert!(verify(&task, None).is_none());
        task.expect_url = Some("#casa-flora".into());
        task.expect_text = vec!["Design".into()];
        let snapshot: Snapshot = serde_json::from_value(serde_json::json!({
            "url": "http://x/#casa-flora", "title": "", "w": 1, "h": 1,
            "text": "Design stays", "scroll": {"y": 0, "height": 1},
            "actions": [], "marker": [], "page_key": [], "guards": {},
            "omitted_actions": 0, "graphics": [],
        }))
        .expect("snapshot");
        assert!(verify(&task, Some(&snapshot)).expect("checks").passed);
        task.expect_text.push("Free cancellation".into());
        assert!(!verify(&task, Some(&snapshot)).expect("checks").passed);
        assert!(!verify(&task, None).expect("checks").passed);
    }
}
