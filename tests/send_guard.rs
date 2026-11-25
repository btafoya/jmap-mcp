//! The send guard end to end, against a fake JMAP server that can be slow, fail with a 502,
//! or refuse with a JMAP error. The real server cannot be made to lose an answer.

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use jmap_mcp::{guard::Guard, jmap::Jmap, tools::*};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

#[derive(Clone, Copy)]
enum Submit {
    SlowOk,
    Http502,
    Forbidden,
}

struct Fake {
    submit: Mutex<Submit>,
    submissions: AtomicUsize,
    base: String,
}

async fn session(State(f): State<Arc<Fake>>) -> Json<Value> {
    Json(json!({
        "capabilities": {"urn:ietf:params:jmap:core": {"maxObjectsInSet": 50, "maxObjectsInGet": 50},
                         "urn:ietf:params:jmap:mail": {}, "urn:ietf:params:jmap:submission": {}},
        "primaryAccounts": {"urn:ietf:params:jmap:mail": "a1"},
        "apiUrl": format!("{}/jmap", f.base)
    }))
}

async fn api(
    State(f): State<Arc<Fake>>,
    Json(req): Json<Value>,
) -> (axum::http::StatusCode, Json<Value>) {
    let call = &req["methodCalls"][0];
    let (name, id) = (call[0].as_str().unwrap(), call[2].clone());
    let ok = |args: Value| {
        (
            axum::http::StatusCode::OK,
            Json(json!({"methodResponses": [[name, args, id]]})),
        )
    };
    match name {
        "Email/get" => ok(
            json!({"list": [{"id": "d1", "keywords": {"$draft": true}, "mailboxIds": {"m2": true},
                                           "from": [{"email": "bob@example.com"}]}]}),
        ),
        "Identity/get" => ok(json!({"list": [{"id": "i1", "email": "bob@example.com"}]})),
        "Mailbox/get" => ok(
            json!({"list": [{"id": "m2", "name": "Drafts", "role": "drafts"}, {"id": "m3", "name": "Sent", "role": "sent"}]}),
        ),
        "EmailSubmission/get" => ok(
            json!({"list": [{"deliveryStatus": {"alice@example.com": {"delivered": "queued", "smtpReply": ""}}}]}),
        ),
        "EmailSubmission/set" => {
            f.submissions.fetch_add(1, Ordering::SeqCst);
            let mode = *f.submit.lock().unwrap();
            match mode {
                Submit::SlowOk => {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    (
                        axum::http::StatusCode::OK,
                        Json(json!({"methodResponses": [
                        ["EmailSubmission/set", {"created": {"s": {"id": "s1", "undoStatus": "pending"}}}, id],
                        ["Email/set", {"updated": {"d1": null}}, id]]})),
                    )
                }
                Submit::Http502 => (axum::http::StatusCode::BAD_GATEWAY, Json(json!({}))),
                Submit::Forbidden => (
                    axum::http::StatusCode::OK,
                    Json(
                        json!({"methodResponses": [["error", {"type": "forbiddenFrom", "description": "nope"}, id]]}),
                    ),
                ),
            }
        }
        other => panic!("unexpected method {other}"),
    }
}

async fn setup(mode: Submit) -> (Arc<Tools>, Arc<Fake>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let fake = Arc::new(Fake {
        submit: Mutex::new(mode),
        submissions: AtomicUsize::new(0),
        base: base.clone(),
    });
    let app = Router::new()
        .route("/.well-known/jmap", get(session))
        .route("/jmap", post(api))
        .with_state(fake.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let jmap = Jmap::connect(&base, "t").await.unwrap();
    (
        Arc::new(Tools {
            jmap,
            guard: Guard::default(),
            allow_write: true,
        }),
        fake,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_duplicate_sends_submit_once() {
    let (t, fake) = setup(Submit::SlowOk).await;
    let calls: Vec<_> = (0..8)
        .map(|_| {
            let t = t.clone();
            tokio::spawn(async move { t.send_draft("d1", None).await })
        })
        .collect();
    let mut sent = 0;
    for c in calls {
        if let Ok(r) = c.await.unwrap() {
            assert!(!r.replayed);
            sent += 1;
        }
    }
    assert_eq!(
        sent, 1,
        "one caller sends, the rest are refused as in progress"
    );
    assert_eq!(fake.submissions.load(Ordering::SeqCst), 1);
    // after it finished, a retry gets the recorded result and still does not submit
    let again = t.send_draft("d1", None).await.unwrap();
    assert!(again.replayed);
    assert_eq!(fake.submissions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn lost_answer_blocks_the_retry_instead_of_sending_again() {
    let (t, fake) = setup(Submit::Http502).await;
    let first = t.send_draft("d1", None).await.unwrap_err().to_string();
    assert!(first.contains("outcome unknown"), "{first}");
    let retry = t.send_draft("d1", None).await.unwrap_err().to_string();
    assert!(retry.contains("Check the Sent folder"), "{retry}");
    assert_eq!(fake.submissions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_definite_refusal_leaves_the_draft_retryable() {
    let (t, fake) = setup(Submit::Forbidden).await;
    let e = t.send_draft("d1", None).await.unwrap_err().to_string();
    assert!(e.contains("forbiddenFrom"), "{e}");
    *fake.submit.lock().unwrap() = Submit::SlowOk;
    assert!(!t.send_draft("d1", None).await.unwrap().replayed);
    assert_eq!(fake.submissions.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_cancelled_send_counts_as_unknown() {
    let (t, _) = setup(Submit::SlowOk).await;
    let t2 = t.clone();
    let h = tokio::spawn(async move { t2.send_draft("d1", None).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    h.abort(); // the client gave up mid-call
    let _ = h.await;
    let e = t.send_draft("d1", None).await.unwrap_err().to_string();
    assert!(e.contains("Check the Sent folder"), "{e}");
}
