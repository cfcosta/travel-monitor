//! The "questions" policy: one Jev request per step, with one choice for
//! the operation and one choice of target per operation. Ported from
//! vs1-browser's `policy.rs` (its upstream prompt shape, the one hosted
//! Jev passed the hotel fixture with), which follows Browser Use's Jev
//! Ultrafast. Changed here: typed requests through `tm-jev`, single
//! candidates resolved without asking, and no text model: a separate
//! value question picks one of the task's known values.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Value, json};
use tm_jev::{Answer, JevError, Question, Request, Response};

use super::Step;
use crate::snapshot::{Action, ActionKind, Snapshot};

const NEXT_ACTION: &str = include_str!("../../assets/next-action.txt");
const TARGET: &str = "Choose the best observed target if the next operation is the one specified in this question. Use the user's entire goal, known values, nearby text, and recent actions. This question chooses only a target for that operation; another question decides which operation to execute. Do not choose a field that already contains the requested value. Choose only an offered element index.";
const VALUE: &str = "Choose which known value to enter in the focused field, judging by the field's label, the page and the goal. Choose none if no known value belongs in this field. Choose only an offered option.";
const RECOVERY: &str = "Actions with no observed effect, or already executed twice in this same state, are temporarily excluded. Fields for which no known value fits are excluded too. Try a different supported control or representation. Graphic labels alone do not prove all plotted values were compared.";

/// The value-question option meaning that no known value fits.
pub(crate) const NONE: &str = "none";
pub(crate) const DONE: &str = "DONE";
pub(crate) const BLOCKED: &str = "BLOCKED";
pub(crate) const OPERATION: &str = "operation";
pub(crate) const VALUE_QUESTION: &str = "value";

/// The actions on offer, indexed for the questions.
#[derive(Debug)]
pub(crate) struct Space {
    /// One entry per observed node, as the model sees it.
    pub elements: Vec<Value>,
    /// Operation → target index → action.
    pub targets: BTreeMap<String, BTreeMap<String, Action>>,
    /// Scroll and wait, offered as operations of their own.
    pub controls: BTreeMap<String, Action>,
}

fn operation_of(kind: ActionKind) -> Option<&'static str> {
    match kind {
        ActionKind::Click => Some("CLICK"),
        ActionKind::Fill => Some("TYPE_TEXT"),
        ActionKind::Select => Some("SELECT"),
        ActionKind::Key => Some("PRESS_KEY"),
        ActionKind::Scroll | ActionKind::Wait => None,
    }
}

/// Groups actions by node (one element index per node) and by
/// operation. Without known values, nothing can be typed, so fields are
/// offered only as clicks.
pub(crate) fn action_space<'a>(
    actions: impl IntoIterator<Item = &'a Action>,
    can_type: bool,
) -> Space {
    let mut elements: Vec<Value> = vec![];
    let mut indices = HashMap::new();
    let mut targets: BTreeMap<String, BTreeMap<String, Action>> =
        BTreeMap::new();
    let mut controls = BTreeMap::new();
    for action in actions {
        let Some(operation) = operation_of(action.kind) else {
            controls.insert(action.id.to_uppercase(), action.clone());
            continue;
        };
        if action.kind == ActionKind::Fill && !can_type {
            continue;
        }
        let Some(node) = action.node else {
            continue;
        };
        let index = *indices.entry(node).or_insert_with(|| {
            let mut element = Map::new();
            if let Some(role) = &action.role {
                element.insert("role".into(), json!(role));
            }
            if let Some(value) = &action.value {
                element.insert("value".into(), json!(value));
            }
            for (key, value) in [
                ("checked", &action.checked),
                ("selected", &action.selected),
                ("expanded", &action.expanded),
            ] {
                if let Some(value) = value {
                    element.insert(key.into(), json!(value));
                }
            }
            element.insert(
                "index".into(),
                json!((elements.len() + 1).to_string()),
            );
            element.insert(
                "label".into(),
                json!(action.label.split(" → ").next().unwrap_or_default()),
            );
            element.insert("operations".into(), json!([]));
            if action.kind == ActionKind::Select {
                element.insert("value".into(), json!(action.current_value));
                element.insert("options".into(), json!([]));
            }
            elements.push(Value::Object(element));
            elements.len() - 1
        });
        let element = &mut elements[index];
        if let Some(operations) = element["operations"].as_array_mut()
            && !operations.contains(&json!(operation))
        {
            operations.push(json!(operation));
        }
        let mut target = (index + 1).to_string();
        if action.kind == ActionKind::Key {
            target = format!(
                "{target}:{}",
                action.key.as_deref().unwrap_or_default()
            );
        }
        if action.kind == ActionKind::Select
            && let Some(options) = element["options"].as_array_mut()
        {
            target = format!("{target}:{}", options.len() + 1);
            options.push(json!({"index": target, "label": action.label,
                "value": action.value}));
        }
        targets
            .entry(operation.to_owned())
            .or_default()
            .insert(target, action.clone());
    }
    Space {
        elements,
        targets,
        controls,
    }
}

/// Whether `action` should be withheld in this state: it had no effect
/// here, or ran twice here within the last 20 steps, or (for a field)
/// no known value fit it here.
fn suppressed(action: &Action, snapshot: &Snapshot, history: &[Step]) -> bool {
    let matching: Vec<&Step> = history
        .iter()
        .rev()
        .take(20)
        .filter(|step| {
            step.before_fingerprint == snapshot.fingerprint
                && step.action.node.is_some()
                && action.node.is_some()
                && step.action.id == action.id
                && step.action.kind == action.kind
                && step.action.label == action.label
                && step.action.key == action.key
                && step.action.value == action.value
        })
        .collect();
    if action.kind == ActionKind::Fill {
        matching.iter().any(|step| step.skipped.is_some())
    } else {
        matching.len() >= 2
            || matching.iter().any(|step| step.page_changed == Some(false))
    }
}

/// The step's request, its action space, and the answers already known
/// (target questions with a single candidate are not asked).
pub(crate) struct Prepared {
    pub request: Request,
    pub space: Space,
    pub known: BTreeMap<String, String>,
}

pub(crate) fn request(
    snapshot: &Snapshot,
    goal: &str,
    history: &[Step],
    values: &BTreeMap<String, String>,
) -> Prepared {
    let mut withheld = vec![];
    let available: Vec<&Action> = snapshot
        .actions
        .iter()
        .filter(|action| {
            let hide = suppressed(action, snapshot, history);
            if hide {
                withheld.push(action.label.clone());
            }
            !hide
        })
        .collect();
    let space = action_space(available, !values.is_empty());
    let mut operations = BTreeMap::new();
    for operation in space.targets.keys() {
        let description = match operation.as_str() {
            "CLICK" => {
                "Click an element, button, menu option, autocomplete suggestion, or calendar day."
            }
            "TYPE_TEXT" => {
                "Enter or replace text in an editable field. The text is one of the known values, chosen next."
            }
            "SELECT" => "Select an observed dropdown value.",
            _ => {
                "Press an offered key in a focusable graphic. Arrow keys can inspect adjacent values; Enter can select the inspected value."
            }
        };
        operations.insert(operation.clone(), json!(description));
    }
    for (id, action) in &space.controls {
        operations.insert(id.clone(), json!(action.label));
    }
    operations.insert(
        DONE.into(),
        json!("Every requirement is visibly satisfied."),
    );
    operations.insert(
        BLOCKED.into(),
        json!("No supported operation can progress."),
    );

    let mut request = Request::new(Value::Null).question(
        OPERATION,
        Question::Choice {
            instructions: json!({"goal": goal, "rules": NEXT_ACTION}),
            criteria: operations,
        },
    );
    let mut known = BTreeMap::new();
    for (operation, candidates) in &space.targets {
        let id = format!("{}_target", operation.to_lowercase());
        if candidates.len() == 1 {
            if let Some(only) = candidates.keys().next() {
                known.insert(id, only.clone());
            }
            continue;
        }
        let criteria = candidates
            .iter()
            .map(|(index, action)| {
                let mut entry = json!({
                    "element": format!("[{index}] {}", action.label),
                    "current_value": action
                        .current_value
                        .as_ref()
                        .or(action.value.as_ref())
                        .cloned()
                        .unwrap_or_default(),
                });
                for (key, value) in [
                    ("role", &action.role),
                    ("checked", &action.checked),
                    ("selected", &action.selected),
                    ("expanded", &action.expanded),
                ] {
                    if let Some(value) = value {
                        entry[key] = json!(value);
                    }
                }
                (index.clone(), entry)
            })
            .collect();
        request = request.question(
            id,
            Question::Choice {
                instructions: json!({"goal": goal, "operation": operation,
                    "rules": [NEXT_ACTION, TARGET]}),
                criteria,
            },
        );
    }
    let mut state = json!({
        "page": {"url": snapshot.url, "title": snapshot.title, "text": snapshot.text},
        "elements": space.elements,
        "recent_actions": recent(history, 10),
        "graphics": snapshot.graphics,
        "recovery": {"temporarily_unavailable": withheld, "instruction": RECOVERY},
    });
    if !values.is_empty() {
        state["known_values"] = json!(values);
    }
    request.state = state;
    Prepared {
        request,
        space,
        known,
    }
}

fn recent(history: &[Step], n: usize) -> Vec<Value> {
    let skip = history.len().saturating_sub(n);
    history[skip..]
        .iter()
        .map(|step| {
            let mut entry = json!({"action": step.action.label,
                "kind": step.action.kind, "text": step.text,
                "page_changed": step.page_changed});
            if let Some(skipped) = &step.skipped {
                entry["skipped"] = json!(skipped);
            }
            entry
        })
        .collect()
}

/// What Jev chose for a step.
#[derive(Debug, Clone)]
pub(crate) struct Decision {
    pub operation: String,
    pub target: Option<String>,
    /// `None` for DONE and BLOCKED.
    pub action: Option<Action>,
    pub confidence: f64,
}

/// Validates the answers against what was offered and resolves the
/// chosen action. Only the chosen operation's target is read.
pub(crate) fn resolve(
    prepared: &Prepared,
    response: &Response,
) -> Result<Decision, JevError> {
    let (operation, confidence) =
        validate_choice(response, OPERATION, &prepared.request)?;
    if let Some(candidates) = prepared.space.targets.get(&operation) {
        let id = format!("{}_target", operation.to_lowercase());
        let target = match prepared.known.get(&id) {
            Some(only) => only.clone(),
            None => validate_choice(response, &id, &prepared.request)?.0,
        };
        let action = candidates.get(&target).cloned().ok_or_else(|| {
            JevError::Malformed(format!("{id}: {target} was not offered"))
        })?;
        return Ok(Decision {
            operation,
            target: Some(target),
            action: Some(action),
            confidence,
        });
    }
    Ok(Decision {
        action: prepared.space.controls.get(&operation).cloned(),
        operation,
        target: None,
        confidence,
    })
}

/// The chosen option of the Choice `id`, checked strictly: it must be
/// one of the offered options, the probabilities must cover exactly the
/// offered options and sum to one, and the choice must be the most
/// probable.
pub(crate) fn validate_choice(
    response: &Response,
    id: &str,
    request: &Request,
) -> Result<(String, f64), JevError> {
    let malformed = |what: &str| JevError::Malformed(format!("{id}: {what}"));
    let Some(Question::Choice { criteria, .. }) = request.questions.get(id)
    else {
        return Err(malformed("not asked"));
    };
    let Some(answer) = response.answers.get(id) else {
        return Err(JevError::MissingAnswer(id.to_owned()));
    };
    let Answer::Choice {
        choice,
        probabilities,
        confidence,
    } = answer
    else {
        return Err(JevError::WrongAnswer {
            id: id.to_owned(),
            expected: "choice",
        });
    };
    if !criteria.contains_key(choice)
        || criteria.len() != probabilities.len()
        || !criteria
            .keys()
            .all(|option| probabilities.contains_key(option))
    {
        return Err(malformed("choice does not match the offered options"));
    }
    let in_range = |p: f64| p.is_finite() && (0.0..=1.0).contains(&p);
    if !in_range(*confidence) {
        return Err(JevError::OutOfRange {
            id: id.to_owned(),
            value: *confidence,
        });
    }
    if !probabilities.values().all(|p| in_range(*p))
        || (probabilities.values().sum::<f64>() - 1.0).abs() >= 0.02
    {
        return Err(malformed("invalid probability distribution"));
    }
    let chosen = probabilities[choice];
    if probabilities.values().any(|p| *p > chosen + 1e-6) {
        return Err(malformed("choice is not the most probable option"));
    }
    Ok((choice.clone(), *confidence))
}

/// The question that replaces vs1's text model: which known value, if
/// any, belongs in the field `action`.
pub(crate) fn value_request(
    goal: &str,
    action: &Action,
    snapshot: &Snapshot,
    history: &[Step],
    values: &BTreeMap<String, String>,
) -> Request {
    let mut options: BTreeMap<String, Value> = values
        .iter()
        .map(|(key, value)| (key.clone(), json!(value)))
        .collect();
    options.insert(NONE.into(), json!("No known value belongs in this field."));
    let text: String = snapshot.text.chars().take(3000).collect();
    Request::new(json!({
        "goal": goal,
        "field": {"label": action.label, "role": action.role,
            "current_value": action.value},
        "page": {"url": snapshot.url, "title": snapshot.title, "text": text},
        "recent_actions": recent(history, 6),
        "known_values": values,
    }))
    .question(
        VALUE_QUESTION,
        Question::Choice {
            instructions: json!({"goal": goal, "field": action.label,
                "rules": VALUE}),
            criteria: options,
        },
    )
}

#[cfg(test)]
mod tests {
    use tm_jev::fake;

    use super::*;

    fn snapshot(actions: Value) -> Snapshot {
        serde_json::from_value(json!({
            "url": "about:blank", "title": "Search", "w": 800, "h": 600,
            "text": "Search", "scroll": {"y": 0, "height": 600},
            "actions": actions, "marker": [], "page_key": [], "guards": {},
            "omitted_actions": 0, "graphics": [], "fingerprint": "same",
        }))
        .expect("snapshot")
    }

    fn page() -> Snapshot {
        snapshot(json!([
            {"id": "e1", "kind": "fill", "label": "Search", "role": "textbox", "value": "", "node": 10},
            {"id": "e2", "kind": "click", "label": "Open Search", "role": "textbox", "node": 10},
            {"id": "e3", "kind": "click", "label": "Go", "role": "button", "node": 20},
            {"id": "wait", "kind": "wait", "label": "Wait for the page to update"},
        ]))
    }

    fn values() -> BTreeMap<String, String> {
        BTreeMap::from([("query".to_owned(), "Lisbon".to_owned())])
    }

    fn choice(options: &[&str], selected: &str) -> Answer {
        Answer::Choice {
            choice: selected.to_owned(),
            probabilities: options
                .iter()
                .map(|o| {
                    (o.to_string(), if *o == selected { 1.0 } else { 0.0 })
                })
                .collect(),
            confidence: 1.0,
        }
    }

    fn response(answers: Vec<(&str, Answer)>, request: &Request) -> Response {
        fake::response(
            answers
                .into_iter()
                .map(|(id, answer)| (id.to_owned(), answer))
                .collect(),
            request,
        )
    }

    fn step(action: &Action, page_changed: Option<bool>) -> Step {
        Step {
            number: 1,
            operation: "CLICK".into(),
            action: action.clone(),
            target: None,
            value_key: None,
            text: None,
            url: "about:blank".into(),
            before_fingerprint: "same".into(),
            page_changed,
            skipped: None,
            elapsed_ms: 0,
        }
    }

    #[test]
    fn one_index_per_node_and_only_the_chosen_target_is_read() {
        let prepared = request(&page(), "Find Lisbon", &[], &values());
        assert_eq!(prepared.space.elements.len(), 2);
        assert!(prepared.space.controls.contains_key("WAIT"));
        // One fill candidate: resolved without asking.
        assert!(!prepared.request.questions.contains_key("type_text_target"));
        assert!(prepared.request.questions.contains_key("click_target"));
        let answers = vec![(
            OPERATION,
            choice(
                &["BLOCKED", "CLICK", "DONE", "TYPE_TEXT", "WAIT"],
                "TYPE_TEXT",
            ),
        )];
        let decision =
            resolve(&prepared, &response(answers, &prepared.request))
                .expect("decision");
        assert_eq!(decision.target.as_deref(), Some("1"));
        assert_eq!(decision.action.expect("action").id, "e1");
    }

    #[test]
    fn an_invented_target_is_rejected() {
        let prepared = request(&page(), "Find Lisbon", &[], &values());
        let operations = ["BLOCKED", "CLICK", "DONE", "TYPE_TEXT", "WAIT"];
        let answers = vec![
            (OPERATION, choice(&operations, "CLICK")),
            ("click_target", choice(&["1", "2", "999"], "999")),
        ];
        assert!(
            resolve(&prepared, &response(answers, &prepared.request)).is_err()
        );
        let answers = vec![(OPERATION, choice(&operations, "CLICK"))];
        assert!(matches!(
            resolve(&prepared, &response(answers, &prepared.request)),
            Err(JevError::MissingAnswer(_))
        ));
    }

    #[test]
    fn the_choice_must_be_the_most_probable_option() {
        let prepared = request(&page(), "Find Lisbon", &[], &values());
        let mut probabilities: BTreeMap<String, f64> =
            ["BLOCKED", "CLICK", "DONE", "TYPE_TEXT", "WAIT"]
                .into_iter()
                .map(|o| (o.to_owned(), 0.0))
                .collect();
        probabilities.insert("CLICK".into(), 0.7);
        probabilities.insert("DONE".into(), 0.3);
        let answer = Answer::Choice {
            choice: "DONE".into(),
            probabilities,
            confidence: 0.5,
        };
        let answers = vec![(OPERATION, answer)];
        assert!(
            resolve(&prepared, &response(answers, &prepared.request)).is_err()
        );
    }

    #[test]
    fn without_known_values_fields_are_only_clickable() {
        let prepared = request(&page(), "Find Lisbon", &[], &BTreeMap::new());
        assert!(!prepared.space.targets.contains_key("TYPE_TEXT"));
        assert!(prepared.request.state.get("known_values").is_none());
    }

    #[test]
    fn keyboard_choices_keep_key_and_node_identity() {
        let page = snapshot(json!([
            {"id": "a", "kind": "key", "node": 3, "label": "Chart → ArrowLeft", "key": "ArrowLeft"},
            {"id": "b", "kind": "key", "node": 3, "label": "Chart → ArrowRight", "key": "ArrowRight"},
        ]));
        let space = action_space(&page.actions, true);
        assert_eq!(
            space.targets["PRESS_KEY"]["1:ArrowRight"].key.as_deref(),
            Some("ArrowRight")
        );
        assert_eq!(space.targets["PRESS_KEY"].len(), 2);
        assert_eq!(space.elements.len(), 1);
    }

    #[test]
    fn select_choices_keep_the_observed_option() {
        let page = snapshot(json!([
            {"id": "a", "kind": "select", "node": 3, "label": "Category → Design", "value": "design"},
            {"id": "b", "kind": "select", "node": 3, "label": "Category → Coastal", "value": "coastal"},
        ]));
        let space = action_space(&page.actions, true);
        assert_eq!(
            space.targets["SELECT"]["1:2"].value.as_deref(),
            Some("coastal")
        );
        assert_eq!(space.elements.len(), 1);
    }

    #[test]
    fn ineffective_actions_are_withheld_only_in_the_same_state() {
        let mut page = page();
        let go = page.actions[2].clone();
        let history = vec![step(&go, Some(false))];
        let prepared = request(&page, "Search", &history, &values());
        assert_eq!(prepared.space.targets["CLICK"].len(), 1);
        assert!(
            prepared
                .request
                .state
                .to_string()
                .contains("temporarily_unavailable")
        );
        assert!(
            prepared.request.state["recovery"]["temporarily_unavailable"][0]
                == "Go"
        );
        page.fingerprint = "changed".into();
        let prepared = request(&page, "Search", &history, &values());
        assert_eq!(prepared.space.targets["CLICK"].len(), 2);
    }

    #[test]
    fn an_action_repeated_twice_in_one_state_is_withheld() {
        let page = page();
        let go = page.actions[2].clone();
        let once = vec![step(&go, Some(true))];
        assert_eq!(
            request(&page, "Search", &once, &values()).space.targets["CLICK"]
                .len(),
            2
        );
        let twice = vec![step(&go, Some(true)), step(&go, Some(true))];
        assert_eq!(
            request(&page, "Search", &twice, &values()).space.targets["CLICK"]
                .len(),
            1
        );
    }

    #[test]
    fn a_field_without_a_fitting_value_is_withheld() {
        let page = page();
        let mut skipped = step(&page.actions[0], None);
        skipped.skipped = Some("no known value fits".into());
        let prepared = request(&page, "Search", &[skipped], &values());
        assert!(!prepared.space.targets.contains_key("TYPE_TEXT"));
    }

    #[test]
    fn the_value_question_offers_every_known_value_and_none() {
        let page = page();
        let request = value_request(
            "Find Lisbon",
            &page.actions[0],
            &page,
            &[],
            &values(),
        );
        let Some(Question::Choice { criteria, .. }) =
            request.questions.get(VALUE_QUESTION)
        else {
            panic!("no value question");
        };
        assert_eq!(criteria.keys().collect::<Vec<_>>(), ["none", "query"]);
        assert_eq!(criteria["query"], "Lisbon");
    }
}
