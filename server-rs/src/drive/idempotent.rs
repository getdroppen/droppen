//! The route half of idempotent sends.
//!
//! [`crate::inject::receipts`] keeps the claims; this decides what a request
//! does with one. Three answers, and the order they are tried in matters:
//!
//! * the id was never seen — this request **owns** it, and runs the send on a
//!   task of its own ([`settle`]), so a client that hangs up mid-request (the
//!   case this whole feature is for) cannot leave the claim half-made;
//! * the id was answered — the answer is **replayed**, marked `duplicate`, and
//!   nothing is enqueued;
//! * the id is being answered right now — the request **waits** for that
//!   answer, briefly, and says `409 in_progress` if it does not come.
//!
//! A claim the first attempt was refused under is dropped again, so a refusal
//! is never replayed: the retry is a fresh attempt.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use gotcha::axum::http::StatusCode;
use serde_json::Value;

use droppen_http::error::{ApiError, Hinted};
use droppen_http::openapi_ext::Optional;
use super::routes::{NewSessionResult, SendMessageResult};
use crate::inject::receipts::{self, Claim, InFlight, Kind, Receipts};
use crate::sessions::Source;

/// How long a retry waits for the attempt it collided with to answer.
pub(crate) const WAIT_FOR_FIRST: Duration = Duration::from_secs(5);
/// How often it looks.
const POLL: Duration = Duration::from_millis(50);

/// The `error` a retry gets while the first attempt is still running.
pub(crate) const IN_PROGRESS: &str = "in_progress";
/// The `error` an id reused for another route or session gets.
pub(crate) const REUSED: &str = "client_message_id_reused";

/// What one request is claiming.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Key<'a> {
    /// The client's `clientMessageId`.
    pub id: &'a str,
    /// Which route.
    pub kind: Kind,
    /// Which CLI.
    pub source: Source,
    /// The session, for a message; `None` for a new session.
    pub session: Option<&'a str>,
}

/// What a request may go on to do.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Begun {
    /// Send the message, under this queue id.
    Owned {
        /// The queue id the message is to travel under.
        qid: String,
    },
    /// Answer this, which is what the first attempt answered.
    Replay(Value),
}

/// A `clientMessageId` is a UUID, which keeps the table to ids a client minted.
pub(crate) fn validate_id(id: &str) -> Result<(), ApiError> {
    if crate::accounts::store::is_uuid(id) {
        Ok(())
    } else {
        Err(ApiError::bad_request("invalid clientMessageId: expected a UUID"))
    }
}

/// Claim `key`, wait out an attempt already running under it, or find the
/// answer it gave.
///
/// `holds_queued` says whether a driver still holds a queue id in its queue:
/// the evidence that an attempt stranded by a crash had enqueued its message
/// after all, in which case its answer is reconstructed rather than the message
/// sent a second time.
pub(crate) async fn begin(receipts: &Receipts, key: Key<'_>, holds_queued: impl Fn(&str) -> bool) -> Result<Begun, ApiError> {
    let started = Instant::now();
    loop {
        let now = receipts::now_ms();
        let claim = receipts
            .claim(key.id, key.kind, key.source, key.session, now)
            .map_err(|error| ApiError::internal(format!("cannot record the message: {error}")))?;
        match claim {
            Claim::Owned { qid } => return Ok(Begun::Owned { qid }),
            Claim::Replay(json) => {
                return serde_json::from_str(&json)
                    .map(Begun::Replay)
                    .map_err(|error| ApiError::internal(format!("cannot read the recorded answer: {error}")));
            }
            Claim::Mismatch => {
                return Err(ApiError::refusal(
                    StatusCode::CONFLICT,
                    Hinted {
                        error: REUSED,
                        hint: "this clientMessageId was already used for a different session or route; mint a new one",
                    },
                ));
            }
            Claim::InFlight(flight) if flight.stale => {
                if let Some(answer) = stranded_answer(key.kind, &flight, &holds_queued) {
                    receipts.complete(key.id, &answer.to_string(), flight.session_id.as_deref(), now);
                    return Ok(Begun::Replay(answer));
                }
                let took = receipts
                    .take_over(key.id, flight.created_ms, now)
                    .map_err(|error| ApiError::internal(format!("cannot record the message: {error}")))?;
                if took {
                    tracing::warn!(qid = %flight.qid, "taking over a send nobody finished; it was never enqueued");
                    return Ok(Begun::Owned { qid: flight.qid });
                }
                // Somebody else took it over between the read and the write:
                // look again, as a retry of *their* attempt.
            }
            Claim::InFlight(_) => {
                if started.elapsed() >= WAIT_FOR_FIRST {
                    return Err(in_progress());
                }
                tokio::time::sleep(POLL).await;
            }
        }
    }
}

/// The `409` a retry gets while the first attempt is still running.
pub(crate) fn in_progress() -> ApiError {
    ApiError::refusal(
        StatusCode::CONFLICT,
        Hinted {
            error: IN_PROGRESS,
            hint: "an earlier attempt with this clientMessageId is still being accepted; ask again shortly",
        },
    )
}

/// What an attempt stranded by a crash answered, when there is evidence it
/// got as far as enqueuing — a driver moved its receipt, or still holds it.
fn stranded_answer(kind: Kind, flight: &InFlight, holds_queued: &impl Fn(&str) -> bool) -> Option<Value> {
    let touched = flight.status != "pending";
    let answer = match kind {
        Kind::Message if touched || holds_queued(&flight.qid) => serde_json::to_value(SendMessageResult {
            accepted: true,
            queued: flight.status != "sent",
            qid: Optional(Some(flight.qid.clone())),
            duplicate: Optional(None),
            answered: Optional(None),
        }),
        Kind::NewSession if touched => serde_json::to_value(NewSessionResult {
            id: flight.session_id.clone()?,
            duplicate: Optional(None),
        }),
        _ => return None,
    };
    Some(answer.expect("answers encode"))
}

/// Run an owned attempt to the end, and write down what it answered.
///
/// On a task of its own, deliberately: a client whose connection drops takes
/// its request future with it, and a send cancelled between "enqueued" and
/// "recorded" is exactly the claim nobody could ever answer. Detached, the send
/// finishes and records itself whatever happened to the socket.
///
/// `work` answers the JSON to replay and, for a new session, the session it
/// made. A refusal abandons the claim — nothing was enqueued under it.
pub(crate) async fn settle<F>(receipts: Arc<Receipts>, id: String, work: F) -> Result<Value, ApiError>
where
    F: Future<Output = Result<(Value, Option<String>), ApiError>> + Send + 'static,
{
    let task = tokio::spawn(async move {
        let outcome = work.await;
        match &outcome {
            Ok((answer, session)) => receipts.complete(&id, &answer.to_string(), session.as_deref(), receipts::now_ms()),
            Err(_) => receipts.abandon(&id),
        }
        outcome.map(|(answer, _)| answer)
    });
    // A panic leaves the claim in flight, which a retry reads as still
    // running until it goes stale: nothing here can say whether the message
    // was enqueued before it, so the answer is the one a retry would get.
    task.await.unwrap_or_else(|_| Err(in_progress()))
}

/// A recorded answer, marked as the replay it is.
pub(crate) fn duplicate(mut answer: Value) -> Value {
    if let Value::Object(object) = &mut answer {
        object.insert("duplicate".into(), Value::Bool(true));
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SESSION: &str = "22222222-2222-4222-8222-222222222222";
    const ID: &str = "33333333-3333-4333-8333-333333333333";

    fn store() -> (tempfile::TempDir, Arc<Receipts>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let receipts = Arc::new(Receipts::new(droppen_instance::db::at(dir.path().join("state.db"))));
        (dir, receipts)
    }

    fn message_key() -> Key<'static> {
        Key {
            id: ID,
            kind: Kind::Message,
            source: Source::Claude,
            session: Some(SESSION),
        }
    }

    /// One attempt of the send route, with `enqueue` standing for the driver.
    async fn attempt(receipts: Arc<Receipts>, enqueues: Arc<AtomicUsize>, delay: Duration, refuse: bool) -> Result<Value, ApiError> {
        match begin(&receipts, message_key(), |_| false).await? {
            Begun::Replay(answer) => Ok(duplicate(answer)),
            Begun::Owned { qid } => {
                settle(receipts, ID.to_string(), async move {
                    tokio::time::sleep(delay).await;
                    if refuse {
                        return Err(ApiError::conflict("session is busy"));
                    }
                    enqueues.fetch_add(1, Ordering::SeqCst);
                    Ok((serde_json::json!({ "accepted": true, "queued": false, "qid": qid }), None))
                })
                .await
            }
        }
    }

    #[tokio::test]
    async fn the_same_id_twice_enqueues_once_and_replays_the_first_answer() {
        let (_dir, receipts) = store();
        let enqueues = Arc::new(AtomicUsize::new(0));
        let first = attempt(receipts.clone(), enqueues.clone(), Duration::ZERO, false).await.expect("first");
        let second = attempt(receipts.clone(), enqueues.clone(), Duration::ZERO, false).await.expect("second");
        assert_eq!(enqueues.load(Ordering::SeqCst), 1, "the retry enqueued nothing");
        assert_eq!(first.get("duplicate"), None);
        assert_eq!(second["duplicate"], true);
        assert_eq!(second["qid"], first["qid"], "the replay names the message the first attempt enqueued");
        assert_eq!(second["queued"], first["queued"]);
    }

    /// Two attempts racing on one id: exactly one gets through, and the other
    /// waits for its answer rather than enqueuing a second copy.
    #[tokio::test]
    async fn two_concurrent_requests_with_one_id_enqueue_once() {
        let (_dir, receipts) = store();
        let enqueues = Arc::new(AtomicUsize::new(0));
        let a = tokio::spawn(attempt(receipts.clone(), enqueues.clone(), Duration::from_millis(300), false));
        let b = tokio::spawn(attempt(receipts.clone(), enqueues.clone(), Duration::from_millis(300), false));
        let (a, b) = (a.await.expect("join").expect("a"), b.await.expect("join").expect("b"));
        assert_eq!(enqueues.load(Ordering::SeqCst), 1);
        let duplicates = [&a, &b]
            .iter()
            .filter(|answer| answer.get("duplicate") == Some(&Value::Bool(true)))
            .count();
        assert_eq!(duplicates, 1, "one owned it, the other replayed it: {a} / {b}");
        assert_eq!(a["qid"], b["qid"]);
    }

    /// A first attempt that outlasts the wait is not answered for: the retry
    /// says so instead of enqueuing.
    #[tokio::test(start_paused = true)]
    async fn a_retry_that_outwaits_the_first_attempt_is_told_it_is_in_progress() {
        let (_dir, receipts) = store();
        let now = receipts::now_ms();
        let claim = receipts.claim(ID, Kind::Message, Source::Claude, Some(SESSION), now).expect("claim");
        assert!(matches!(claim, Claim::Owned { .. }));
        let error = begin(&receipts, message_key(), |_| false).await.expect_err("still running");
        assert_eq!(error.status(), StatusCode::CONFLICT);
        assert_eq!(error.body()["error"], IN_PROGRESS);
    }

    /// A refused attempt is not recorded: the retry is a fresh attempt, and
    /// it can succeed.
    #[tokio::test]
    async fn a_refusal_is_not_recorded() {
        let (_dir, receipts) = store();
        let enqueues = Arc::new(AtomicUsize::new(0));
        let refused = attempt(receipts.clone(), enqueues.clone(), Duration::ZERO, true)
            .await
            .expect_err("refused");
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        let lookup = receipts
            .lookup(Source::Claude, SESSION, &[ID.to_string()], receipts::now_ms())
            .expect("lookup");
        assert_eq!(lookup.unknown, vec![ID.to_string()], "nothing was recorded under the id");

        let retried = attempt(receipts.clone(), enqueues.clone(), Duration::ZERO, false).await.expect("retry");
        assert_eq!(retried.get("duplicate"), None, "a fresh attempt, not a replay of the refusal");
        assert_eq!(enqueues.load(Ordering::SeqCst), 1);
    }

    /// The point of running the send detached: the request that started it
    /// can vanish, and the claim is still answered.
    #[tokio::test]
    async fn a_dropped_request_still_records_its_answer() {
        let (_dir, receipts) = store();
        let enqueues = Arc::new(AtomicUsize::new(0));
        let request = tokio::spawn(attempt(receipts.clone(), enqueues.clone(), Duration::from_millis(200), false));
        tokio::time::sleep(Duration::from_millis(50)).await;
        request.abort();
        let retried = attempt(receipts.clone(), enqueues.clone(), Duration::ZERO, false).await.expect("retry");
        assert_eq!(retried["duplicate"], true, "the retry waited for the orphaned attempt and replayed it");
        assert_eq!(enqueues.load(Ordering::SeqCst), 1);
    }

    /// A new session's id is the server's, so a retry is answered with the
    /// session the first attempt made rather than a second one.
    #[tokio::test]
    async fn a_new_session_retried_under_one_id_is_the_same_session() {
        let (_dir, receipts) = store();
        let started = Arc::new(AtomicUsize::new(0));
        let key = Key {
            id: ID,
            kind: Kind::NewSession,
            source: Source::Codex,
            session: None,
        };
        let mut answers = Vec::new();
        for _ in 0..2 {
            let answer = match begin(&receipts, key, |_| false).await.expect("begin") {
                Begun::Replay(answer) => duplicate(answer),
                Begun::Owned { .. } => {
                    let started = started.clone();
                    settle(receipts.clone(), ID.to_string(), async move {
                        let n = started.fetch_add(1, Ordering::SeqCst);
                        let id = format!("thread-{n}");
                        Ok((serde_json::json!({ "id": id }), Some(id)))
                    })
                    .await
                    .expect("settle")
                }
            };
            answers.push(answer);
        }
        assert_eq!(started.load(Ordering::SeqCst), 1, "one session was started");
        assert_eq!(answers[0]["id"], "thread-0");
        assert_eq!(answers[1]["id"], "thread-0", "the retry names the same session");
        assert_eq!(answers[1]["duplicate"], true);
    }

    /// A claim from a server that died mid-send is answered from what the
    /// driver it reached says, rather than sent a second time.
    #[tokio::test]
    async fn a_stranded_claim_a_driver_holds_is_answered_not_resent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        let before = Receipts::new(droppen_instance::db::at(&path));
        let then = receipts::now_ms() - receipts::STALE_AFTER_MS - 1;
        let qid = match before.claim(ID, Kind::Message, Source::Claude, Some(SESSION), then).expect("claim") {
            Claim::Owned { qid } => qid,
            other => panic!("{other:?}"),
        };
        let held = qid.clone();
        match begin(&before, message_key(), move |candidate| candidate == held).await.expect("begin") {
            Begun::Replay(answer) => {
                assert_eq!(answer["qid"], qid.as_str());
                assert_eq!(answer["queued"], true);
            }
            other => panic!("expected the stranded attempt to be answered, got {other:?}"),
        }

        // And one nobody holds is taken over, under the same queue id.
        let other = "44444444-4444-4444-8444-444444444444";
        let stranded = match before.claim(other, Kind::Message, Source::Claude, Some(SESSION), then).expect("claim") {
            Claim::Owned { qid } => qid,
            claim => panic!("{claim:?}"),
        };
        let key = Key { id: other, ..message_key() };
        assert_eq!(begin(&before, key, |_| false).await.expect("begin"), Begun::Owned { qid: stranded });
    }

    /// What a stranded attempt is answered with is stored and replayed to
    /// clients of any version, so it is pinned byte for byte.
    #[test]
    fn a_stranded_answer_is_recorded_as_pinned() {
        let flight = |status: &str, session_id: Option<&str>| InFlight {
            qid: "q-1".into(),
            status: status.into(),
            session_id: session_id.map(str::to_string),
            created_ms: 0,
            stale: true,
        };
        let recorded = |kind: Kind, flight: InFlight, held: bool| stranded_answer(kind, &flight, &|_: &str| held).map(|answer| answer.to_string());

        assert_eq!(
            recorded(Kind::Message, flight("pending", None), true).as_deref(),
            Some(r#"{"accepted":true,"queued":true,"qid":"q-1"}"#)
        );
        assert_eq!(
            recorded(Kind::Message, flight("sent", None), false).as_deref(),
            Some(r#"{"accepted":true,"queued":false,"qid":"q-1"}"#)
        );
        assert_eq!(recorded(Kind::Message, flight("pending", None), false), None);
        assert_eq!(
            recorded(Kind::NewSession, flight("sent", Some("thread-0")), false).as_deref(),
            Some(r#"{"id":"thread-0"}"#)
        );
        assert_eq!(recorded(Kind::NewSession, flight("sent", None), false), None);
        assert_eq!(recorded(Kind::NewSession, flight("pending", Some("thread-0")), true), None);
    }

    /// The two refusals a retry can get, byte for byte: older clients tell
    /// them apart by `error` and show `hint`.
    #[tokio::test]
    async fn a_reused_or_running_id_is_refused_as_pinned() {
        let in_progress = in_progress();
        assert_eq!(in_progress.status(), StatusCode::CONFLICT);
        assert_eq!(
            in_progress.body().to_string(),
            r#"{"error":"in_progress","hint":"an earlier attempt with this clientMessageId is still being accepted; ask again shortly"}"#
        );

        let (_dir, receipts) = store();
        assert!(matches!(begin(&receipts, message_key(), |_| false).await, Ok(Begun::Owned { .. })));
        let elsewhere = Key {
            session: Some("55555555-5555-4555-8555-555555555555"),
            ..message_key()
        };
        let reused = begin(&receipts, elsewhere, |_| false).await.expect_err("another session's id");
        assert_eq!(reused.status(), StatusCode::CONFLICT);
        assert_eq!(
            reused.body().to_string(),
            r#"{"error":"client_message_id_reused","hint":"this clientMessageId was already used for a different session or route; mint a new one"}"#
        );
    }

    #[test]
    fn only_a_uuid_is_a_client_message_id() {
        assert!(validate_id(ID).is_ok());
        assert!(validate_id("").is_err());
        assert!(validate_id("local:1").is_err());
        assert!(validate_id(&"a".repeat(36)).is_err());
    }
}
