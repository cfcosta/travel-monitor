//! `FakeJev`, the test double callers use instead of `TypeSafe`.

#![allow(clippy::unwrap_used, reason = "a failed unwrap fails the test")]
#![allow(
    clippy::disallowed_methods,
    reason = "`#[tokio::test]` blocks on its runtime: a test is a synchronous entry point"
)]

use std::collections::BTreeMap;

use serde_json::json;
use tm_jev::{
    Answer,
    Jev,
    JevError,
    Question,
    Request,
    fake::{FakeJev, response},
};

fn request() -> Request {
    Request::new(json!({"page": "checkout"}))
        .question("done", Question::noul("Is the search finished?"))
        .question("blocked", Question::noul("Is there a captcha?"))
}

/// `nouls` answers every question as a Noul, and records each request
/// in order, shared across clones.
#[tokio::test]
async fn nouls_answer_and_record() {
    let jev = FakeJev::nouls(|id| if id == "done" { 0.9 } else { 0.1 });
    let clone = jev.clone();
    let response = clone.ask(&request()).await.unwrap();
    assert_eq!(response.noul("done").unwrap(), 0.9);
    assert_eq!(response.noul("blocked").unwrap(), 0.1);
    assert_eq!(response.model, "jev-fake");
    assert_eq!(jev.requests(), vec![request()]);
}

/// A Noul answer is still checked: the fake does not bypass validation.
#[tokio::test]
async fn fake_answers_are_checked() {
    let jev = FakeJev::nouls(|_| 2.0);
    let response = jev.ask(&request()).await.unwrap();
    assert!(matches!(
        response.noul("done"),
        Err(JevError::OutOfRange { .. })
    ));
    assert!(matches!(
        response.choice("done"),
        Err(JevError::WrongAnswer { .. })
    ));
}

/// A function can answer with anything, errors included; a bigger
/// request is billed more input tokens.
#[tokio::test]
async fn new_answers_from_a_function() {
    let jev = FakeJev::new(|request| {
        if request.questions.contains_key("fail") {
            return Err(JevError::Status(503));
        }
        let answers = BTreeMap::from([(
            "site".to_owned(),
            Answer::Choice {
                choice: "latam".to_owned(),
                probabilities: BTreeMap::from([("latam".to_owned(), 1.0)]),
                confidence: 0.7,
            },
        )]);
        Ok(response(answers, request))
    });
    let small = Request::new("s").question(
        "site",
        Question::choice("Which site?", [("latam", "LATAM")]),
    );
    let big = Request::new("a much longer state than the other").question(
        "site",
        Question::choice("Which site?", [("latam", "LATAM")]),
    );
    let small_usage = jev.ask(&small).await.unwrap().usage();
    let answer = jev.ask(&big).await.unwrap();
    assert_eq!(answer.choice("site").unwrap(), ("latam", 0.7));
    assert!(answer.usage().input_tokens > small_usage.input_tokens);
    assert!(answer.usage().cost > small_usage.cost);
    assert_eq!(
        jev.ask(&Request::new("x").question("fail", Question::noul("?")))
            .await,
        Err(JevError::Status(503))
    );
    assert_eq!(jev.requests().len(), 3);
}
