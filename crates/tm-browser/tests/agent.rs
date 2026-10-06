//! The Jev policy, driven by a scripted `FakeJev` that reads each
//! request's state the way a model would, against vs1-browser's local
//! task fixture in a real headless Chromium.

#![allow(
    clippy::disallowed_methods,
    reason = "#[tokio::test] expands to block_on: a test is the runtime's entry point"
)]

mod common;

use std::{collections::BTreeMap, time::Duration};

use common::{Reply, Server, chromium};
use serde_json::Value;
use tm_browser::agent::{self, Status, Task};
use tm_jev::{
    Answer,
    JevError,
    Question,
    Request,
    Response,
    fake::{self, FakeJev},
};

const TASKS: &str = include_str!("fixtures/tasks.html");
const LOAD: Duration = Duration::from_secs(15);

/// A Choice answer for `id`: all probability on `chosen`, over exactly
/// the offered options.
fn choose(request: &Request, id: &str, chosen: &str) -> (String, Answer) {
    let Some(Question::Choice { criteria, .. }) = request.questions.get(id)
    else {
        panic!("{id} was not asked");
    };
    assert!(
        criteria.contains_key(chosen),
        "{chosen} not offered for {id}"
    );
    let probabilities = criteria
        .keys()
        .map(|o| (o.clone(), if o == chosen { 1.0 } else { 0.0 }))
        .collect();
    (
        id.to_owned(),
        Answer::Choice {
            choice: chosen.to_owned(),
            probabilities,
            confidence: 0.9,
        },
    )
}

/// The offered target of `id` whose description mentions `label`.
fn target(request: &Request, id: &str, label: &str) -> Option<String> {
    let Some(Question::Choice { criteria, .. }) = request.questions.get(id)
    else {
        return None;
    };
    criteria
        .iter()
        .find(|(_, d)| d["element"].as_str().is_some_and(|e| e.contains(label)))
        .map(|(k, _)| k.clone())
}

fn element<'a>(state: &'a Value, label: &str) -> Option<&'a Value> {
    state["elements"]
        .as_array()?
        .iter()
        .find(|e| e["label"].as_str().is_some_and(|l| l.trim() == label))
}

fn respond(request: &Request, answers: Vec<(String, Answer)>) -> Response {
    fake::response(answers.into_iter().collect(), request)
}

/// Fills the support form the way the fixture expects, field by field.
fn support_form_oracle(request: &Request) -> Result<Response, JevError> {
    let state = &request.state;
    if request.questions.contains_key("value") {
        let label = state["field"]["label"].as_str().unwrap_or_default();
        let key = match label.trim() {
            "Name" => "name",
            "Email" => "email",
            "Message" => "message",
            _ => "none",
        };
        return Ok(respond(request, vec![choose(request, "value", key)]));
    }
    let title = state["page"]["title"].as_str().unwrap_or_default();
    if title.starts_with("Support request sent") {
        return Ok(respond(
            request,
            vec![choose(request, "operation", "DONE")],
        ));
    }
    let mut answers = vec![];
    for field in ["Name", "Email", "Message"] {
        let empty = element(state, field)
            .is_some_and(|e| e["value"].as_str() == Some(""));
        if empty {
            answers.push(choose(request, "operation", "TYPE_TEXT"));
            if let Some(index) = target(request, "type_text_target", field) {
                answers.push(choose(request, "type_text_target", &index));
            }
            return Ok(respond(request, answers));
        }
    }
    if element(state, "Topic").is_some_and(|e| e["value"] != "Billing") {
        answers.push(choose(request, "operation", "SELECT"));
        let index = target(request, "select_target", "Billing")
            .expect("Billing is offered");
        answers.push(choose(request, "select_target", &index));
        return Ok(respond(request, answers));
    }
    let consent =
        element(state, "I accept the privacy terms").expect("privacy checkbox");
    let label = if consent["checked"] == "false" {
        "I accept the privacy terms"
    } else {
        "Send"
    };
    answers.push(choose(request, "operation", "CLICK"));
    let index = target(request, "click_target", label).expect("offered");
    answers.push(choose(request, "click_target", &index));
    Ok(respond(request, answers))
}

#[tokio::test(flavor = "multi_thread")]
async fn jev_fills_and_submits_the_support_form() {
    let Some((chromium, _profile)) = chromium().await else {
        return;
    };
    let server = Server::start(|_| Reply::html(TASKS)).await;
    let page = chromium.browser().new_page().await.expect("page");
    page.navigate(&server.url("/tasks.html?case=contact-form"), LOAD)
        .await
        .expect("navigate");

    let jev = FakeJev::new(support_form_oracle);
    let mut task = Task::new(
        "Send a Billing support request as Ana Souza (ana@example.com) saying the card was charged twice. Accept the privacy terms.",
    )
    .value("name", "Ana Souza")
    .value("email", "ana@example.com")
    .value("message", "My card was charged twice.")
    .value("origin", "São Paulo");
    task.max_steps = 20;
    task.expect_url = Some("#confirmation".into());
    task.expect_text =
        vec!["Ana Souza".into(), "Billing".into(), "Accepted".into()];

    let outcome = agent::run(&page, &jev, &task).await;
    assert_eq!(outcome.status, Status::Done, "{outcome:#?}");
    assert!(outcome.succeeded(), "{:#?}", outcome.verification);

    let submission = page
        .eval("window.__taskState.submission")
        .await
        .expect("submission");
    assert_eq!(submission["name"], "Ana Souza");
    assert_eq!(submission["email"], "ana@example.com");
    assert_eq!(submission["topic"], "Billing");
    assert_eq!(submission["message"], "My card was charged twice.");
    assert_eq!(submission["hasAcceptedPrivacyTerms"], true);

    let trace = &outcome.trace;
    let typed: BTreeMap<_, _> = trace
        .steps
        .iter()
        .filter_map(|s| Some((s.value_key.clone()?, s.text.clone()?)))
        .collect();
    assert_eq!(typed.len(), 3, "{typed:?}");
    assert_eq!(typed["email"], "ana@example.com");
    assert_eq!(trace.steps.len(), 6, "{:#?}", trace.steps);
    assert!(trace.steps.iter().all(|s| s.page_changed == Some(true)));
    // Six operation decisions, three value decisions, one DONE.
    assert_eq!(trace.decisions.len(), 10);
    assert!(trace.jev_usage.input_tokens > 0);
    assert!(
        trace
            .final_url
            .as_deref()
            .is_some_and(|u| u.ends_with("#confirmation"))
    );

    // The trace serializes, and known values reach Jev as state.
    let json = serde_json::to_value(&outcome).expect("serializable");
    assert_eq!(json["status"], "done");
    let first = &jev.requests()[0];
    assert_eq!(first.state["known_values"]["origin"], "São Paulo");
    chromium.shutdown().await.expect("shutdown");
}

const COUPON: &str = r#"<title>Checkout</title>
<label>Coupon code <input id="coupon"></label>
<button onclick="out.textContent = 'Applied ' + (++window.clicks) + ' times'">Apply</button>
<p id="out"></p>
<script>window.clicks = 0</script>"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_field_with_no_fitting_value_is_skipped_and_withheld() {
    let Some((chromium, _profile)) = chromium().await else {
        return;
    };
    let server = Server::start(|_| Reply::html(COUPON)).await;
    let page = chromium.browser().new_page().await.expect("page");
    page.navigate(&server.url("/"), LOAD)
        .await
        .expect("navigate");

    let jev = FakeJev::new(|request| {
        if request.questions.contains_key("value") {
            return Ok(respond(
                request,
                vec![choose(request, "value", "none")],
            ));
        }
        let Some(Question::Choice { criteria, .. }) =
            request.questions.get("operation")
        else {
            panic!("no operation question");
        };
        let operation = if criteria.contains_key("TYPE_TEXT") {
            "TYPE_TEXT"
        } else {
            "BLOCKED"
        };
        Ok(respond(
            request,
            vec![choose(request, "operation", operation)],
        ))
    });
    let task = Task::new("Apply a coupon").value("origin", "São Paulo");
    let outcome = agent::run(&page, &jev, &task).await;
    assert_eq!(outcome.status, Status::Blocked, "{outcome:#?}");
    let step = &outcome.trace.steps[0];
    assert!(step.skipped.is_some());
    assert_eq!(step.text, None);
    assert_eq!(page.eval("coupon.value").await.expect("value"), "");
    let requests = jev.requests();
    assert_eq!(requests.len(), 3, "operation, value, then operation");
    assert!(
        requests[2].state["recovery"]["temporarily_unavailable"]
            .as_array()
            .is_some_and(|a| a.iter().any(|l| l == "Coupon code"))
    );
    chromium.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn budgets_and_jev_failures_end_the_run() {
    let Some((chromium, _profile)) = chromium().await else {
        return;
    };
    let server = Server::start(|_| Reply::html(COUPON)).await;
    let page = chromium.browser().new_page().await.expect("page");
    page.navigate(&server.url("/"), LOAD)
        .await
        .expect("navigate");

    let clicker = FakeJev::new(|request| {
        let index = target(request, "click_target", "Apply");
        let mut answers = vec![choose(request, "operation", "CLICK")];
        if let Some(index) = index {
            answers.push(choose(request, "click_target", &index));
        }
        Ok(respond(request, answers))
    });
    let mut task = Task::new("Keep applying");
    task.max_steps = 2;
    let outcome = agent::run(&page, &clicker, &task).await;
    assert_eq!(outcome.status, Status::Exhausted, "{outcome:#?}");
    assert_eq!(outcome.trace.steps.len(), 2);
    assert_eq!(page.eval("window.clicks").await.expect("clicks"), 2);

    let broken = FakeJev::new(|_| Err(JevError::Status(503)));
    let outcome = agent::run(&page, &broken, &task).await;
    assert_eq!(outcome.status, Status::Failed);
    assert!(outcome.reason.as_deref().is_some_and(|r| r.contains("503")));
    assert_eq!(
        outcome.trace.decisions[0].error.as_deref(),
        Some("Jev answered with status 503")
    );

    let invented = FakeJev::new(|request| {
        Ok(respond(
            request,
            vec![(
                "operation".to_owned(),
                Answer::Choice {
                    choice: "SUBMIT_FORM".into(),
                    probabilities: BTreeMap::from([(
                        "SUBMIT_FORM".into(),
                        1.0,
                    )]),
                    confidence: 1.0,
                },
            )],
        ))
    });
    let outcome = agent::run(&page, &invented, &task).await;
    assert_eq!(outcome.status, Status::Failed, "{outcome:#?}");
    assert_eq!(page.eval("window.clicks").await.expect("clicks"), 2);
    chromium.shutdown().await.expect("shutdown");
}
