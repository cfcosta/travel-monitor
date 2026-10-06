//! `TypeSafe`, the HTTP client, against a local server that plays back
//! scripted responses and records the requests it gets.

#![allow(clippy::unwrap_used, reason = "a failed unwrap fails the test")]
#![allow(
    clippy::disallowed_methods,
    reason = "`#[tokio::test]` blocks on its runtime: a test is a synchronous entry point"
)]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tm_jev::{
    Cost,
    Jev,
    JevError,
    Question,
    Request,
    RetryPolicy,
    TypeSafe,
    Usage,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

/// One scripted HTTP response.
struct Reply {
    status: u16,
    headers: Vec<(&'static str, &'static str)>,
    body: String,
}

fn ok(body: Value) -> Reply {
    Reply {
        status: 200,
        headers: vec![],
        body: body.to_string(),
    }
}

fn status(status: u16, headers: Vec<(&'static str, &'static str)>) -> Reply {
    Reply {
        status,
        headers,
        body: "{\"error\":\"echoes the request\"}".into(),
    }
}

/// A request the server got: its headers (lowercased names) and body.
#[derive(Debug, Clone)]
struct Seen {
    headers: Vec<(String, String)>,
    body: Value,
}

/// Serves `replies` in order, one per connection, on a free port.
async fn serve(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    tokio::spawn(async move {
        for reply in replies {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 4096];
            let (head, body_start) = loop {
                let n = socket.read(&mut chunk).await.unwrap();
                buffer.extend_from_slice(&chunk[..n]);
                if let Some(end) =
                    buffer.windows(4).position(|w| w == b"\r\n\r\n")
                {
                    break (
                        String::from_utf8(buffer[..end].to_vec()).unwrap(),
                        end + 4,
                    );
                }
            };
            let headers: Vec<(String, String)> = head
                .lines()
                .skip(1)
                .filter_map(|line| line.split_once(':'))
                .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_owned()))
                .collect();
            let length: usize = headers
                .iter()
                .find(|(k, _)| k == "content-length")
                .map_or(0, |(_, v)| v.parse().unwrap());
            while buffer.len() < body_start + length {
                let n = socket.read(&mut chunk).await.unwrap();
                buffer.extend_from_slice(&chunk[..n]);
            }
            let body = serde_json::from_slice(
                &buffer[body_start..body_start + length],
            )
            .unwrap();
            record.lock().unwrap().push(Seen { headers, body });
            let mut response = format!(
                "HTTP/1.1 {} X\r\ncontent-length: {}\r\ncontent-type: application/json\r\nconnection: close\r\n",
                reply.status,
                reply.body.len()
            );
            for (name, value) in reply.headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("\r\n");
            response.push_str(&reply.body);
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        }
    });
    (url, seen)
}

fn fast_retries() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3,
        base: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
    }
}

fn request() -> Request {
    Request::new(json!({"ticket": "refund please"}))
        .question("refund", Question::noul("Does this ask for a refund?"))
        .question(
            "team",
            Question::choice(
                "Which team?",
                [("billing", "money"), ("support", "rest")],
            ),
        )
        .question("urgency", Question::score("How urgent?", ["low", "high"]))
}

fn answers() -> Value {
    json!({
        "model": "jev-1.13.0",
        "answers": {
            "refund": {"type": "noul", "noul": 0.97},
            "team": {"type": "choice", "choice": "billing", "probabilities": {"billing": 0.9, "support": 0.1}, "confidence": 0.8},
            "urgency": {"type": "score", "score": 0.6, "probabilities": {"0": 0.4, "1": 0.6}, "confidence": 0.2},
        },
        "usage": {"input_tokens": 1_000_000, "output_tokens": 20},
    })
}

/// The request goes as TypeSafe documents it, with the key as a bearer
/// token and the client's model; the answers come back typed, and the
/// usage is priced per input token.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_round_trips() {
    let (url, seen) = serve(vec![ok(answers())]).await;
    let jev = TypeSafe::new("sk-test").url(url).model("jev-preview");
    let response = jev.ask(&request()).await.unwrap();
    assert_eq!(response.noul("refund").unwrap(), 0.97);
    assert_eq!(response.choice("team").unwrap(), ("billing", 0.8));
    assert_eq!(response.score("urgency").unwrap(), (0.6, 0.2));
    assert_eq!(
        response.usage(),
        Usage {
            input_tokens: 1_000_000,
            output_tokens: 20,
            cost: Cost { usd: 0.042 },
        }
    );

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0]
            .headers
            .contains(&("authorization".into(), "Bearer sk-test".into()))
    );
    assert_eq!(
        seen[0].body,
        json!({
            "model": "jev-preview",
            "state": {"ticket": "refund please"},
            "questions": {
                "refund": {"type": "noul", "instructions": "Does this ask for a refund?"},
                "team": {"type": "choice", "instructions": "Which team?", "criteria": {"billing": "money", "support": "rest"}},
                "urgency": {"type": "score", "instructions": "How urgent?", "criteria": ["low", "high"]},
            },
        })
    );
}

/// 429, 503 and 529 are retried, up to the policy's attempts; the last
/// status is the error once the policy runs out.
#[tokio::test(flavor = "multi_thread")]
async fn overload_is_retried_under_the_policy() {
    let (url, seen) = serve(vec![
        status(429, vec![("retry-after", "0")]),
        status(503, vec![]),
        ok(answers()),
    ])
    .await;
    let jev = TypeSafe::new("k").url(url).retry(fast_retries());
    assert!(jev.ask(&request()).await.is_ok());
    assert_eq!(seen.lock().unwrap().len(), 3);

    let (url, seen) = serve(vec![
        status(529, vec![]),
        status(529, vec![]),
        status(529, vec![]),
    ])
    .await;
    let jev = TypeSafe::new("k").url(url).retry(fast_retries());
    assert_eq!(jev.ask(&request()).await, Err(JevError::Status(529)));
    assert_eq!(seen.lock().unwrap().len(), 3);
}

/// Other errors are not retried, and their body is not kept.
#[tokio::test(flavor = "multi_thread")]
async fn other_errors_fail_at_once() {
    let (url, seen) = serve(vec![status(401, vec![]), ok(answers())]).await;
    let jev = TypeSafe::new("bad").url(url).retry(fast_retries());
    let error = jev.ask(&request()).await.unwrap_err();
    assert_eq!(error, JevError::Status(401));
    assert!(!error.to_string().contains("echoes"));
    assert_eq!(seen.lock().unwrap().len(), 1);
}

/// A body that is not TypeSafe's JSON is an error, not a guess.
#[tokio::test(flavor = "multi_thread")]
async fn malformed_responses_are_errors() {
    let (url, _) = serve(vec![Reply {
        status: 200,
        headers: vec![],
        body: "{\"answers\": 3}".into(),
    }])
    .await;
    let jev = TypeSafe::new("k").url(url);
    assert!(matches!(
        jev.ask(&request()).await,
        Err(JevError::Malformed(_))
    ));
}

/// Answers are checked when read: a missing one, one of the wrong kind,
/// or a probability outside `[0, 1]` is an error.
#[test]
fn answers_are_checked() {
    let response: tm_jev::Response = serde_json::from_value(json!({
        "model": "jev-1.13.0",
        "answers": {
            "high": {"type": "noul", "noul": 1.2},
            "team": {"type": "choice", "choice": "a", "probabilities": {"a": 1.0}, "confidence": 1.0},
        },
        "usage": {"input_tokens": 1},
    }))
    .unwrap();
    assert!(matches!(
        response.noul("high"),
        Err(JevError::OutOfRange { .. })
    ));
    assert!(matches!(
        response.noul("team"),
        Err(JevError::WrongAnswer { .. })
    ));
    assert!(matches!(
        response.noul("gone"),
        Err(JevError::MissingAnswer(_))
    ));
    assert!(matches!(
        response.score("team"),
        Err(JevError::WrongAnswer { .. })
    ));
}

/// The key never shows in debug output, which shows the endpoint and
/// the model instead.
#[test]
fn the_key_stays_secret() {
    let jev = TypeSafe::new("sk-very-secret").model("jev-preview");
    assert_eq!(
        format!("{jev:?}"),
        "TypeSafe { url: \"https://api.typesafe.ai/v1/systemone\", model: \"jev-preview\", .. }"
    );
}

/// A key from the environment is trimmed; a missing or blank one is an
/// error.
#[test]
fn keys_come_trimmed_or_not_at_all() {
    use tm_jev::{MissingApiKey, api_key};
    assert_eq!(api_key(Some(" sk-1\n".into())), Ok("sk-1".into()));
    assert_eq!(api_key(Some("  ".into())), Err(MissingApiKey));
    assert_eq!(api_key(None), Err(MissingApiKey));
    assert_eq!(MissingApiKey.to_string(), "TYPESAFE_API_KEY is not set");
}

/// Every error says what went wrong, without the request.
#[test]
fn errors_explain_themselves() {
    let cases = [
        (
            JevError::Transport("reset".into()),
            "Jev request failed: reset",
        ),
        (JevError::Status(503), "Jev answered with status 503"),
        (
            JevError::Malformed("eof".into()),
            "Jev's response is malformed: eof",
        ),
        (
            JevError::MissingAnswer("q".into()),
            "Jev gave no answer for q",
        ),
        (
            JevError::WrongAnswer {
                id: "q".into(),
                expected: "noul",
            },
            "Jev's answer for q is not a noul",
        ),
        (
            JevError::OutOfRange {
                id: "q".into(),
                value: 1.5,
            },
            "Jev's answer for q is out of range: 1.5",
        ),
    ];
    for (error, text) in cases {
        assert_eq!(error.to_string(), text);
    }
}

/// A key file is trimmed, as the daemon's key file usually ends in a
/// newline; a missing, blank or empty file is an error that names the
/// path and never the contents.
#[test]
fn keys_come_from_files_trimmed_or_not_at_all() {
    use tm_jev::{ApiKeyFileError, api_key_from_file};
    let dir = tempfile::tempdir().unwrap();

    let key = dir.path().join("key");
    std::fs::write(&key, "  sk-from-file\n").unwrap();
    assert_eq!(api_key_from_file(&key).unwrap(), "sk-from-file");
    assert_eq!(
        format!("{:?}", TypeSafe::from_key_file(&key).unwrap()),
        "TypeSafe { url: \"https://api.typesafe.ai/v1/systemone\", model: \"jev-latest\", .. }"
    );

    for contents in ["", " \n\t\n"] {
        let blank = dir.path().join("blank");
        std::fs::write(&blank, contents).unwrap();
        let error = api_key_from_file(&blank).unwrap_err();
        assert!(
            matches!(&error, ApiKeyFileError::Empty { path } if *path == blank)
        );
        assert_eq!(
            error.to_string(),
            format!("the Jev API key file {} is empty", blank.display())
        );
    }

    let missing = dir.path().join("missing");
    let error = api_key_from_file(&missing).unwrap_err();
    assert!(
        matches!(&error, ApiKeyFileError::Read { path, .. } if *path == missing)
    );
    assert!(
        error.to_string().starts_with(&format!(
            "cannot read the Jev API key file {}: ",
            missing.display()
        )),
        "{error}"
    );
    assert!(TypeSafe::from_key_file(&missing).is_err());
}

/// A server that is not there is a transport error, not retried, and
/// its message leaves the URL out.
#[tokio::test(flavor = "multi_thread")]
async fn unreachable_servers_are_transport_errors() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let url = format!("http://{address}/v1/private-path");
    let jev = TypeSafe::new("k").url(url).retry(fast_retries());
    let error = jev.ask(&request()).await.unwrap_err();
    let JevError::Transport(message) = error else {
        panic!("{error:?}");
    };
    assert!(!message.contains("private-path"), "{message}");
}

/// A `retry-after` hint past the policy's cap waits only the cap.
#[tokio::test(flavor = "multi_thread")]
async fn retry_after_is_capped_by_the_policy() {
    let (url, seen) = serve(vec![
        status(503, vec![("retry-after", "3600")]),
        ok(answers()),
    ])
    .await;
    let jev = TypeSafe::new("k").url(url).retry(fast_retries());
    let started = std::time::Instant::now();
    assert!(jev.ask(&request()).await.is_ok());
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(seen.lock().unwrap().len(), 2);
}

/// Usage adds up field by field, and its total counts both directions.
#[test]
fn usage_adds_up() {
    let mut usage = Usage {
        input_tokens: 10,
        output_tokens: 1,
        cost: Cost { usd: 0.5 },
    };
    usage += &Usage {
        input_tokens: 5,
        output_tokens: 2,
        cost: Cost { usd: 0.25 },
    };
    assert_eq!(
        usage,
        Usage {
            input_tokens: 15,
            output_tokens: 3,
            cost: Cost { usd: 0.75 },
        }
    );
    assert_eq!(usage.total_tokens(), 18);
}
