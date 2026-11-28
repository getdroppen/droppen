//! Unit tests for session-driving routes.

use super::*;
use crate::attachments::ATTACHMENT_MISSING;
use crate::{fake, Drive};
use droppen_instance::paths::Paths;
use std::sync::Arc;

/// The routes' own refusals, byte for byte: clients act on `error`, show
/// `hint`, and the settled question's screen reads `question`.
#[test]
fn the_session_refusals_are_written_as_pinned() {
    let external = external_session("this session was started elsewhere; continue it where it was started".to_string());
    assert_eq!(external.status(), gotcha::axum::http::StatusCode::CONFLICT);
    assert_eq!(
        external.body().to_string(),
        r#"{"error":"external_session","hint":"this session was started elsewhere; continue it where it was started"}"#
    );

    let question = SessionQuestion {
        request_id: "r-1".into(),
        tool_use_id: Nullable(None),
        questions: Vec::new(),
        state: QuestionState::Answered,
        answers: Nullable(None),
        response: Nullable(Some("yes".into())),
        answered_by: Nullable(Some("alice".into())),
        answered_at: Nullable(Some("2026-01-01T00:00:00.000Z".into())),
        created_at: "2026-01-01T00:00:00.000Z".into(),
    };
    let row = serde_json::to_string(&question).expect("a question");
    let settled = already_settled(question);
    assert_eq!(settled.status(), gotcha::axum::http::StatusCode::CONFLICT);
    assert_eq!(
        settled.body().to_string(),
        format!(r#"{{"error":"question_settled","hint":"alice already answered it","question":{row}}}"#)
    );

    let restart = queue_result(droppen_provider::QueueOutcome::NeedsRestart {
        differences: "another model".into(),
    })
    .expect_err("a refusal");
    assert_eq!(restart.status(), gotcha::axum::http::StatusCode::CONFLICT);
    assert_eq!(
        restart.body().to_string(),
        r#"{"error":"that message asks for different settings, so it needs a new process rather than a place in this one's queue","hint":"it wants another model, and a process is only replaced between turns. The session will restart for it when the turn in flight ends — or press Stop, and it will start sooner."}"#
    );
}

/// A drive on the fake provider and reader ([`fake`]).
fn drive() -> (tempfile::TempDir, Arc<Drive>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_, drive) = fake::drive(dir.path());
    (dir, drive)
}

/// The drive routes' state over [`drive`]'s, with a session list of its own.
fn state() -> (tempfile::TempDir, DriveState) {
    let (dir, drive) = drive();
    let global = feed::global::GlobalFeed::new();
    let state = DriveState {
        paths: drive.paths.clone(),
        drive,
        refresh: droppen_http::refresh::Refresh::new(global.clone()),
        global,
    };
    (dir, state)
}

#[test]
fn a_flag_value_that_could_read_as_a_flag_is_refused() {
    assert!(validate_flags(ModelRule::NAMED, Some("claude-opus-4.5"), Some("high"), Some("acceptEdits")).is_ok());
    assert!(validate_flags(ModelRule::NAMED, Some("--add-dir"), None, None).is_err());
    assert!(validate_flags(ModelRule::NAMED, None, Some("High"), None).is_err(), "effort is lowercase only");
    assert!(validate_flags(ModelRule::NAMED, None, None, Some("-x")).is_err());
    assert!(validate_flags(ModelRule::NAMED, Some(&"m".repeat(200)), None, None).is_err());
}

/// `claude --model claude-opus-4-8[1m]` is how the CLI is asked for the
/// long-context variant of a model that has one, so a picker that offers it
/// has to be able to send it. The brackets are the whole point, and the one
/// thing that must not follow from allowing them is a value a shell would
/// read as anything but a word.
///
/// They used to get through by accident: the old pattern's `(?-u:\w)` was
/// written inside a character class, where `?-u` is a range rather than a
/// flag group, so `[`, `]`, `\` and `` ` `` were all in it — and so was an
/// *unbalanced* bracket. Hence the second half of this test.
#[test]
fn a_long_context_variant_tag_is_a_legal_model_name() {
    assert!(validate_flags(ModelRule::NAMED, Some("claude-opus-4-8[1m]"), None, None).is_ok());
    assert!(validate_flags(ModelRule::NAMED, Some("opus[1m]"), None, None).is_ok());
    assert!(validate_flags(ModelRule::NAMED, Some("claude-opus-5"), None, None).is_ok());
    assert!(validate_flags(ModelRule::NAMED, Some("claude-opus-4.5"), None, None).is_ok(), "a dotted name still passes");
    assert!(
        validate_flags(ModelRule::NAMED, Some("anthropic.claude-opus-5:0"), None, None).is_ok(),
        "and a prefixed one"
    );

    for hostile in [
        "opus[1m",
        "opus1m]",
        "opus[]",
        "opus[1m][1m]",
        "[1m]",
        "opus[$(id)]",
        "opus[a;b]",
        "opus[*]",
        "opus[1m] --dangerously-skip-permissions",
        "opus[1m]x",
        "opus[`id`]",
        "opus[a|b]",
        "opus[a b]",
        "opus[a\nb]",
        // Everything the accidental `?-u` range used to let through.
        r"opus\bad",
        "opus`id`",
        "opus^x",
        "opus@x",
        "opus(x)",
    ] {
        assert!(validate_flags(ModelRule::NAMED, Some(hostile), None, None).is_err(), "{hostile} must be refused");
    }
}

#[test]
fn a_prompt_must_be_non_empty_and_bounded() {
    assert!(validate_text("hi").is_ok());
    assert!(validate_text("").is_err());
    assert!(validate_text(&"x".repeat(100_001)).is_err());
}

/// An upload on disk, as `POST /api/uploads` leaves one: `uploads/<dir>/<name>`.
fn uploaded(paths: &Paths, name: &str) -> String {
    let path = paths.uploads_dir().join("abc").join(name);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("uploads dir");
    std::fs::write(&path, b"x").expect("upload");
    path.to_string_lossy().into_owned()
}

#[test]
fn only_paths_inside_the_uploads_directory_may_be_attached() {
    let _dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(_dir.path().to_path_buf());
    let uploads = paths.uploads_dir();
    let images = vec![uploaded(&paths, "a.png")];
    assert_eq!(
        resolve_attachments(&paths, images.clone(), vec![]).expect("an upload").0,
        images
    );
    assert!(resolve_attachments(&paths, vec!["/etc/passwd".to_string()], vec![]).is_err());
    assert!(
        resolve_attachments(
            &paths,
            vec![],
            vec![uploads.join("../../.ssh/id_rsa").to_string_lossy().into_owned()]
        )
        .is_err(),
        "a traversal out of the uploads dir is not an upload"
    );
}

/// An upload that is not there any more — swept with the old ones, or a
/// draft's attachment kept past that — is refused with a reason of its own,
/// and so is anything at an upload's path that is not a regular file or
/// that a symlink takes out of the uploads directory.
#[test]
fn an_attachment_must_still_be_a_file_in_the_uploads_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path().to_path_buf());
    let uploads = paths.uploads_dir();
    let code = |result: Result<(Vec<String>, Vec<String>), ApiError>| {
        let error = result.expect_err("refused");
        (error.status().as_u16(), error.body()["error"].as_str().map(str::to_string))
    };
    let missing = (409, Some(ATTACHMENT_MISSING.to_string()));

    // Before the uploads directory was ever made…
    assert!(!uploads.exists());
    let gone = uploads.join("abc/gone.png").to_string_lossy().into_owned();
    assert_eq!(code(resolve_attachments(&paths, vec![gone.clone()], vec![])), missing);
    // …and after.
    let real = uploaded(&paths, "kept.txt");
    assert_eq!(
        code(resolve_attachments(&paths, vec![], vec![real.clone(), gone])),
        missing,
        "one gone refuses the message"
    );
    assert_eq!(
        code(resolve_attachments(
            &paths,
            vec![],
            vec![uploads.join("abc").to_string_lossy().into_owned()]
        )),
        missing,
        "a directory is not a file"
    );
    let refused = resolve_attachments(
        &paths,
        vec![uploads.join("abc/also-gone.png").to_string_lossy().into_owned()],
        vec![],
    )
    .expect_err("refused");
    assert_eq!(
        refused.body()["hint"],
        "an attachment is no longer on this device; attach it again",
        "{:?}",
        refused.body()
    );

    #[cfg(unix)]
    {
        let secret = dir.path().join("secret.txt");
        std::fs::write(&secret, b"s").expect("secret");
        let link = uploads.join("abc/link.txt");
        std::os::unix::fs::symlink(&secret, &link).expect("symlink");
        let error = resolve_attachments(&paths, vec![], vec![link.to_string_lossy().into_owned()]).expect_err("refused");
        assert_eq!(
            (error.status().as_u16(), error.body()["error"].as_str()),
            (409, Some("attachment paths must come from /api/uploads")),
            "a symlink out of the uploads directory is not an upload"
        );
    }
    assert!(resolve_attachments(&paths, vec![], vec![real]).is_ok());
}

/// The message route refuses a missing upload with the same 409, before it
/// looks for the session at all.
#[tokio::test]
async fn a_message_with_an_attachment_that_is_gone_is_refused() {
    let (_home, state) = state();
    let gone = state.drive.paths.uploads_dir().join("abc/gone.png").to_string_lossy().into_owned();
    let body = |images: Vec<String>| SendMessage {
        text: "look".into(),
        images: Optional(Some(images)),
        files: Optional(None),
        model: Optional(None),
        effort: Optional(None),
        permission: Optional(None),
        account_id: None,
        client_message_id: Optional(None),
    };
    let id = "99999999-9999-4999-8999-999999999999".to_string();
    let error = send_message(
        State(state.clone()),
        MaybeIdentity(None),
        ApiPath((fake::SOURCE, id.clone())),
        ApiJson(body(vec![gone])),
    )
    .await
    .expect_err("refused");
    assert_eq!(
        (error.status().as_u16(), error.body()["error"].as_str()),
        (409, Some(ATTACHMENT_MISSING))
    );

    // On disk, it passes; the next thing said no is the session nobody made.
    let path = state.drive.paths.uploads_dir().join("abc/here.png");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("uploads dir");
    std::fs::write(&path, b"x").expect("upload");
    let error = send_message(
        State(state.clone()),
        MaybeIdentity(None),
        ApiPath((fake::SOURCE, id)),
        ApiJson(body(vec![path.to_string_lossy().into_owned()])),
    )
    .await
    .expect_err("no such session");
    assert_eq!(error.status().as_u16(), 404);
}

/// A `POST /api/sessions/new` body as a client sends it, with attachments.
fn new_session_body(cwd: &str, images: Option<Vec<String>>, files: Option<Vec<String>>) -> NewSession {
    NewSession {
        source: fake::SOURCE,
        cwd: cwd.to_string(),
        text: "look at this".to_string(),
        images: Optional(images),
        files: Optional(files),
        model: Optional(None),
        effort: Optional(None),
        permission: Optional(None),
        account_id: None,
        account_from: Optional(None),
        client_message_id: Optional(None),
        acp_agent: Optional(None),
    }
}

/// The first message takes attachments in the message route's own shape,
/// and a body without them — every older client's — still reads.
#[test]
fn a_new_session_takes_the_message_routes_attachment_fields() {
    let older: NewSession = serde_json::from_str(r#"{"source":"claude","cwd":"/w","text":"hi"}"#).expect("parse");
    assert!(older.images.is_none() && older.files.is_none(), "absent stays absent");

    let body = r#"{"source":"codex","cwd":"/w","text":"hi","images":["/u/a.png"],"files":["/u/b.txt"]}"#;
    let new: NewSession = serde_json::from_str(body).expect("parse");
    let message: SendMessage = serde_json::from_str(body).expect("the message route reads the same fields");
    assert_eq!(new.images.0, Some(vec!["/u/a.png".to_string()]));
    assert_eq!(new.files.0, Some(vec!["/u/b.txt".to_string()]));
    assert_eq!((new.images.0, new.files.0), (message.images.0, message.files.0));
}

/// The route, end to end up to the spawn: an attachment the uploads
/// directory did not hand out is refused with the message route's `409`
/// before anything is started, a traversal out of it included, and one it
/// did hand out passes the gate — on to the working-directory check, which
/// stops this test short of starting a CLI.
#[tokio::test]
async fn a_new_sessions_attachments_must_come_from_the_uploads_directory() {
    let (home, state) = state();
    let uploads = state.drive.paths.uploads_dir();
    let missing_cwd = home.path().join("no-such-project").to_string_lossy().into_owned();
    let refused = |error: ApiError| (error.status().as_u16(), error.body()["error"].as_str().map(str::to_string));
    let not_an_upload = Some("attachment paths must come from /api/uploads".to_string());

    // Not from `/api/uploads` at all.
    let outside = new_session_body(&missing_cwd, Some(vec!["/etc/passwd".into()]), None);
    let error = new_session(State(state.clone()), ApiJson(outside)).await.expect_err("refused");
    assert_eq!(refused(error), (409, not_an_upload.clone()));

    // A traversal that starts inside the uploads directory and leaves it.
    let escape = uploads.join("abc/../../../.ssh/id_rsa").to_string_lossy().into_owned();
    let traversal = new_session_body(&missing_cwd, None, Some(vec![escape]));
    let error = new_session(State(state.clone()), ApiJson(traversal)).await.expect_err("refused");
    assert_eq!(refused(error), (409, not_an_upload.clone()));

    // The uploads directory itself is not an upload.
    let bare = new_session_body(&missing_cwd, Some(vec![uploads.to_string_lossy().into_owned()]), None);
    let error = new_session(State(state.clone()), ApiJson(bare)).await.expect_err("refused");
    assert_eq!(refused(error), (409, not_an_upload));

    // An upload that is no longer on the device: its own reason, the draft kept.
    let gone = uploads.join("abc/gone.png").to_string_lossy().into_owned();
    let missing = new_session_body(&missing_cwd, Some(vec![gone]), None);
    let error = new_session(State(state.clone()), ApiJson(missing)).await.expect_err("refused");
    assert_eq!(refused(error), (409, Some(ATTACHMENT_MISSING.to_string())));

    // Uploaded paths pass, and the next thing said no is the directory.
    std::fs::create_dir_all(uploads.join("abc")).expect("uploads dir");
    std::fs::write(uploads.join("abc/a.png"), b"x").expect("upload");
    std::fs::write(uploads.join("abc/notes.txt"), b"x").expect("upload");
    let image = uploads.join("abc/a.png").to_string_lossy().into_owned();
    let file = uploads.join("abc/notes.txt").to_string_lossy().into_owned();
    let valid = new_session_body(&missing_cwd, Some(vec![image]), Some(vec![file]));
    let error = new_session(State(state.clone()), ApiJson(valid)).await.expect_err("no such directory");
    let (status, said) = refused(error);
    assert_eq!(status, 400);
    assert!(said.unwrap_or_default().starts_with("not a directory"), "past the attachment gate");

    // And a named attempt refused for its attachments claims nothing: the
    // same id with the attachment fixed is a first attempt, not a replay.
    let mut named = new_session_body(&missing_cwd, Some(vec!["/etc/passwd".into()]), None);
    named.client_message_id = Optional(Some("6f1c2a52-3b7e-4d1f-9a0c-2b8f1e4d5c6a".into()));
    let error = new_session(State(state.clone()), ApiJson(named.clone())).await.expect_err("refused");
    assert_eq!(refused(error).0, 409);
    named.images = Optional(None);
    let error = new_session(State(state.clone()), ApiJson(named)).await.expect_err("no such directory");
    assert_eq!(refused(error).0, 400, "answered afresh, not with the recorded refusal");
}

/// An expired question whose answer becomes a message carries the chat
/// marker, and a chat never carries the stored permission mode.
#[test]
fn an_expired_answers_options_pin_a_chat_and_leave_a_session_alone() {
    let choice = || ComposerChoice {
        permission: Optional(Some("bypassPermissions".into())),
        ..ComposerChoice::default()
    };
    let chat = expired_answer_options(true, choice());
    assert!(chat.chat);
    assert_eq!(chat.permission, None, "a chat is never restarted with a permission mode");

    let session = expired_answer_options(false, choice());
    assert!(!session.chat);
    assert_eq!(session.permission.as_deref(), Some("bypassPermissions"), "sessions keep their mode");
}

#[test]
fn an_absent_account_id_is_not_a_null_one() {
    let absent: SendMessage = serde_json::from_str(r#"{"text":"hi"}"#).expect("parse");
    assert_eq!(AccountChoice::from_request(absent.account_id), AccountChoice::Unset);

    let null: SendMessage = serde_json::from_str(r#"{"text":"hi","accountId":null}"#).expect("parse");
    assert_eq!(AccountChoice::from_request(null.account_id), AccountChoice::Local);

    let named: SendMessage = serde_json::from_str(r#"{"text":"hi","accountId":"abc"}"#).expect("parse");
    assert_eq!(AccountChoice::from_request(named.account_id), AccountChoice::Account("abc".into()));

    let new: NewSession = serde_json::from_str(r#"{"source":"claude","cwd":"/w","text":"hi","accountId":null}"#).expect("parse");
    assert_eq!(AccountChoice::from_request(new.account_id), AccountChoice::Local);
}

/// An answer is bounded like any prompt, before anything is looked up —
/// so a session that does not even exist answers `400`, not `404`.
#[tokio::test]
async fn an_answer_too_long_for_a_prompt_is_refused_before_anything_else() {
    let (_dir, drive) = drive();
    let huge = Answer {
        response: Some("x".repeat(questions::MAX_ANSWER_CHARS + 1)),
        ..Answer::default()
    };
    let error = settle_question(&drive, fake::SOURCE, "no-such-session", "req", Reply::Answer(huge), None)
        .await
        .expect_err("refused");
    assert_eq!(error.status(), gotcha::axum::http::StatusCode::BAD_REQUEST);
    let error = settle_question(&drive, fake::SOURCE, "no-such-session", "req", Reply::Dismiss, None)
        .await
        .expect_err("missing");
    assert_eq!(
        error.status(),
        gotcha::axum::http::StatusCode::NOT_FOUND,
        "a dismissal has nothing to bound"
    );
}

/// An expired question's answer goes out as a message that also names the
/// question, so an answer inside the bound can make a message outside it.
/// It is refused as what it would be — a prompt too long — before the
/// record moves or anything is sent, and the question stays answerable.
#[tokio::test]
async fn an_expired_answer_too_long_as_a_message_is_refused_and_stays_answerable() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, drive) = drive();
    let id = "99999999-9999-4999-8999-999999999999";
    // A session of this server's own: the fake reader's.
    fake::session(&drive.paths, id, "hi\n");

    let asked = "q".repeat(60_000);
    let questions_json = serde_json::json!([{ "question": asked, "header": "Long", "options": [{ "label": "a" }], "multiSelect": false }]);
    let store = drive.injections.host.questions();
    store.open("req-long", id, Some("toolu_long"), &questions_json, questions::now_ms());
    assert!(store.close("req-long", QuestionState::Expired, questions::now_ms()));

    let answer = Answer {
        answers: BTreeMap::from([(asked.clone(), "a".repeat(50_000))]),
        ..Answer::default()
    };
    assert!(questions::check_answer_size(&answer).is_ok(), "the answer itself is within the bound");
    let error = settle_question(&drive, fake::SOURCE, id, "req-long", Reply::Answer(answer), None)
        .await
        .expect_err("refused");
    assert_eq!(error.status(), gotcha::axum::http::StatusCode::BAD_REQUEST);
    assert!(error.body().to_string().contains("too long to send as a message"), "{}", error.body());
    assert_eq!(
        store.get("req-long").map(|stored| stored.state),
        Some(QuestionState::Expired),
        "nothing moved, so a shorter answer can still be given"
    );
}

// ------------------------------------------------- pages and the first unread off the newest parse

/// `line {i}` for every `i` in `range`, one per line: user texts to the fake reader.
fn lines(range: std::ops::Range<usize>) -> String {
    range.map(|i| format!("line {i}\n")).collect()
}

/// What a live CLI does to its transcript.
fn append(file: &std::path::Path, text: &str) {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(file)
        .expect("open")
        .write_all(text.as_bytes())
        .expect("append");
}

/// The session feed's parse at the file's signature now, which is also
/// what the cache then holds.
async fn parse_now(drive: &Drive, file: &std::path::Path) -> (String, Arc<droppen_sessions::loader::Transcript>) {
    let sig = file_signature(file).await.expect("sig");
    let transcript = drive.transcripts.load(&drive.paths, fake::SOURCE, file, &sig).await;
    (sig, transcript)
}

/// The signature of the parse the cache holds for `file`.
fn cached_sig(drive: &Drive, file: &std::path::Path) -> Option<String> {
    drive.transcripts.newest_cached(file).map(|(sig, _)| sig)
}

/// Whether the cache's parse of `file` is of the file as it is now: it was
/// read again.
async fn read_again(drive: &Drive, file: &std::path::Path) -> bool {
    cached_sig(drive, file) == file_signature(file).await
}

async fn history(state: &DriveState, id: &str, before: usize, fp: Option<String>, since: Option<usize>) -> Result<SessionHistoryPage, ApiError> {
    history_from(state, id, before, fp, since, None).await
}

/// [`history`] reaching back to `from`.
async fn history_from(
    state: &DriveState,
    id: &str,
    before: usize,
    fp: Option<String>,
    since: Option<usize>,
    from: Option<usize>,
) -> Result<SessionHistoryPage, ApiError> {
    let query = HistoryQuery {
        before: before as u32,
        fp,
        since: since.map(|since| since as u32),
        from: from.map(|from| from as u32),
    };
    session_history(State(state.clone()), ApiPath((fake::SOURCE, id.to_string())), ApiQuery(query))
        .await
        .map(|Json(page)| page)
}

fn texts(items: &[impl std::borrow::Borrow<TranscriptItem>]) -> Vec<&str> {
    items.iter().map(|item| item.borrow().text.as_str()).collect()
}

/// A live session is appended to between the feed's parse and the click
/// on "load earlier": the page is cut from the parse already held, which
/// gives exactly the page a fresh read gives, without reading the file.
#[tokio::test]
async fn a_history_page_is_cut_from_the_newest_parse_after_an_append() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, state) = state();
    let id = "history-after-an-append";
    let file = fake::session(&state.paths, id, &lines(0..300));
    // The window the feed sent: the last two hundred items.
    let (held_sig, held) = parse_now(&state.drive, &file).await;
    let start_fp = held.fp_at(100).expect("in range");
    append(&file, &lines(300..310));
    let now = file_signature(&file).await.expect("sig");
    assert_ne!(now, held_sig, "the file moved on");

    let page = history(&state, id, 100, Some(start_fp.clone()), None).await.expect("a page");
    assert_eq!(page.sig, held_sig, "read off the parse already held");
    assert_eq!(cached_sig(&state.drive, &file), Some(held_sig.clone()), "and nothing was parsed");
    assert_eq!((page.start_index, page.end_index, page.has_more_before), (0, 100, false));
    assert_eq!(texts(&page.items), texts(&held.items[..100]));
    assert_eq!(page.start_fp, held.fp_at(0).expect("the seed"));
    assert_eq!(page.total_items, 300, "the total at the page's own signature, which a client never lowers its own to");

    // A window that trimmed its start asks with its resume marker instead.
    let by_marker = history(&state, id, 100, Some(held.prefix_fp()), Some(held.len())).await.expect("a page");
    assert_eq!((by_marker.sig.as_str(), by_marker.start_index), (held_sig.as_str(), 0));

    // What a fresh read of the file gives for the same request.
    droppen_sessions::loader::forget_transcript(&file);
    let fresh = history(&state, id, 100, Some(start_fp), None).await.expect("a page");
    assert_eq!(fresh.sig, now, "read again, with nothing held");
    assert_eq!(texts(&fresh.items), texts(&page.items));
    assert_eq!(
        (fresh.start_index, fresh.end_index, fresh.has_more_before, &fresh.start_fp),
        (page.start_index, page.end_index, page.has_more_before, &page.start_fp)
    );
    assert_eq!(fresh.total_items, 310);
}

/// A transcript rewritten rather than appended to: the parse held cannot
/// vouch for the page, the file is read again, and the client is told its
/// window has to be replaced, as before. Rewritten shorter, the signature
/// says so before any fingerprint is compared; rewritten and parsed since
/// (the feed re-reads every change), the fingerprint does.
#[tokio::test]
async fn a_rewritten_transcript_is_read_again_and_its_page_refused() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, state) = state();
    let id = "history-rewritten";
    let file = fake::session(&state.paths, id, &lines(0..300));
    let (_, held) = parse_now(&state.drive, &file).await;
    let start_fp = held.fp_at(100).expect("in range");
    let conflict = |result: Result<SessionHistoryPage, ApiError>| {
        let error = result.expect_err("refused");
        assert_eq!(error.status(), gotcha::axum::http::StatusCode::CONFLICT);
        assert!(error.body().to_string().contains("transcript_changed"), "{}", error.body());
    };

    // Replaced by a shorter transcript, nothing parsed since.
    std::fs::write(&file, (0..250).map(|i| format!("other {i}\n")).collect::<String>()).expect("rewrite");
    conflict(history(&state, id, 100, Some(start_fp.clone()), None).await);
    assert!(read_again(&state.drive, &file).await);

    // Replaced by a longer one, which the feed has parsed since.
    std::fs::write(&file, (0..400).map(|i| format!("another {i}\n")).collect::<String>()).expect("rewrite");
    parse_now(&state.drive, &file).await;
    conflict(history(&state, id, 100, Some(start_fp), None).await);
    conflict(history(&state, id, 100, Some(held.prefix_fp()), Some(held.len())).await);
}

/// The parse held being unable to prove the anchor is no reason to refuse
/// a page: a client whose window is newer than it is served from a fresh
/// read, exactly as before.
#[tokio::test]
async fn a_page_the_parse_held_cannot_prove_is_read_fresh() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, state) = state();
    let id = "history-newer-window";
    let file = fake::session(&state.paths, id, &lines(0..300));
    parse_now(&state.drive, &file).await;
    append(&file, &lines(300..400));
    // A window cut from a parse of all four hundred, which the cache does not hold.
    let newer = state
        .drive
        .transcripts
        .read_uncached(&state.paths, fake::SOURCE, &file)
        .await
        .expect("readable");
    let page = history(&state, id, 300, Some(newer.prefix_fp()), Some(newer.len())).await.expect("a page");
    let now = file_signature(&file).await.expect("sig");
    assert_eq!((page.sig.as_str(), page.start_index, page.end_index), (now.as_str(), 100, 300));
    assert_eq!(texts(&page.items), texts(&newer.items[100..300]));
}

/// A jump to an item far back asks for everything from it to its window
/// in one request: one answer, proven by the window's start fingerprint
/// like a page, that places by its own start fingerprint like a page.
#[tokio::test]
async fn a_ranged_history_request_is_one_answer_back_to_from() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, state) = state();
    let id = "history-ranged";
    let file = fake::session(&state.paths, id, &lines(0..3000));
    let (sig, held) = parse_now(&state.drive, &file).await;
    // The window the feed sent: the last two hundred items.
    let start_fp = held.fp_at(2800).expect("in range");

    let range = history_from(&state, id, 2800, Some(start_fp.clone()), None, Some(1000)).await.expect("a range");
    assert_eq!((range.start_index, range.end_index, range.has_more_before), (1000, 2800, true));
    assert_eq!(texts(&range.items), texts(&held.items[1000..2800]));
    assert_eq!(range.start_fp, held.fp_at(1000).expect("in range"), "what the next page down is asked for with");
    assert_eq!((range.sig.as_str(), range.total_items), (sig.as_str(), 3000));

    // A trimmed window proves it with its resume marker, as a page.
    let by_marker = history_from(&state, id, 2800, Some(held.prefix_fp()), Some(held.len()), Some(1000)).await.expect("a range");
    assert_eq!((by_marker.start_index, by_marker.end_index), (1000, 2800));

    // A `from` inside the page is the page.
    let near = history_from(&state, id, 2800, Some(start_fp), None, Some(2750)).await.expect("a page");
    assert_eq!((near.start_index, near.end_index, near.items.len()), (2600, 2800, feed::SESSION_PAGE_SIZE));
}

/// A live session appended to since the feed's parse: the range is cut
/// from the parse already held, as a page is, and is exactly what a fresh
/// read gives.
#[tokio::test]
async fn a_range_is_cut_from_the_newest_parse_after_an_append() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, state) = state();
    let id = "history-ranged-after-an-append";
    let file = fake::session(&state.paths, id, &lines(0..3000));
    let (held_sig, held) = parse_now(&state.drive, &file).await;
    let start_fp = held.fp_at(2800).expect("in range");
    append(&file, &lines(3000..3050));
    let now = file_signature(&file).await.expect("sig");
    assert_ne!(now, held_sig, "the file moved on");

    let range = history_from(&state, id, 2800, Some(start_fp.clone()), None, Some(1500)).await.expect("a range");
    assert_eq!(range.sig, held_sig, "read off the parse already held");
    assert_eq!(cached_sig(&state.drive, &file), Some(held_sig.clone()), "and nothing was parsed");
    assert_eq!((range.start_index, range.end_index, range.total_items), (1500, 2800, 3000));
    assert_eq!(texts(&range.items), texts(&held.items[1500..2800]));

    droppen_sessions::loader::forget_transcript(&file);
    let fresh = history_from(&state, id, 2800, Some(start_fp), None, Some(1500)).await.expect("a range");
    assert_eq!((fresh.sig.as_str(), fresh.total_items), (now.as_str(), 3050), "read again, with nothing held");
    assert_eq!(texts(&fresh.items), texts(&range.items));
    assert_eq!(
        (fresh.start_index, fresh.end_index, fresh.has_more_before, &fresh.start_fp),
        (range.start_index, range.end_index, range.has_more_before, &range.start_fp)
    );
}

/// Further back than the cap, the answer is the newest part of the range,
/// and the next request picks up from its start.
#[tokio::test]
async fn a_ranged_history_request_past_the_cap_is_its_newest_part() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, state) = state();
    let id = "history-ranged-capped";
    let file = fake::session(&state.paths, id, &lines(0..3000));
    let (_, held) = parse_now(&state.drive, &file).await;

    let first = history_from(&state, id, 2800, held.fp_at(2800), None, Some(0)).await.expect("a range");
    assert_eq!(first.items.len(), feed::MAX_HISTORY_ITEMS);
    assert_eq!((first.start_index, first.end_index, first.has_more_before), (800, 2800, true));
    let rest = history_from(&state, id, first.start_index, Some(first.start_fp.clone()), None, Some(0))
        .await
        .expect("the rest");
    assert_eq!((rest.start_index, rest.end_index, rest.has_more_before), (0, 800, false));
    assert_eq!(texts(&rest.items), texts(&held.items[..800]));
}

/// A range is refused exactly when a page would be: its anchor no longer
/// holds — rewritten shorter, which the signature says, or rewritten and
/// parsed since, which the fingerprint does
/// (`a_rewritten_transcript_is_read_again_and_its_page_refused`).
#[tokio::test]
async fn a_ranged_history_request_on_a_rewritten_transcript_is_refused() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, state) = state();
    let id = "history-ranged-rewritten";
    let file = fake::session(&state.paths, id, &lines(0..1000));
    let (_, held) = parse_now(&state.drive, &file).await;
    let start_fp = held.fp_at(800).expect("in range");
    let conflict = |result: Result<SessionHistoryPage, ApiError>| {
        let error = result.expect_err("refused");
        assert_eq!(error.status(), gotcha::axum::http::StatusCode::CONFLICT);
        assert!(error.body().to_string().contains("transcript_changed"), "{}", error.body());
    };

    std::fs::write(&file, (0..900).map(|i| format!("o {i}\n")).collect::<String>()).expect("rewrite");
    conflict(history_from(&state, id, 800, Some(start_fp.clone()), None, Some(0)).await);

    std::fs::write(&file, (0..1200).map(|i| format!("other {i}\n")).collect::<String>()).expect("rewrite");
    parse_now(&state.drive, &file).await;
    conflict(history_from(&state, id, 800, Some(start_fp), None, Some(0)).await);
    conflict(history_from(&state, id, 800, Some(held.prefix_fp()), Some(held.len()), Some(0)).await);
}

/// A request without `from` is the one page it always was, however far
/// back the window starts.
#[tokio::test]
async fn a_history_request_without_from_is_one_page() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, state) = state();
    let id = "history-one-page";
    let file = fake::session(&state.paths, id, &lines(0..3000));
    let (_, held) = parse_now(&state.drive, &file).await;
    let page = history(&state, id, 2800, held.fp_at(2800), None).await.expect("a page");
    assert_eq!((page.start_index, page.end_index, page.has_more_before), (2600, 2800, true));
    assert_eq!(texts(&page.items), texts(&held.items[2600..2800]));
    assert_eq!(page.start_fp, held.fp_at(2600).expect("in range"));
    let unchecked = history(&state, id, 150, None, None).await.expect("a page");
    assert_eq!((unchecked.start_index, unchecked.end_index, unchecked.has_more_before), (0, 150, false));
}

/// `n` user lines, then `count` agent messages stamped in minute `minute`,
/// then `tail` more user lines.
fn with_answers(n: usize, minute: usize, count: usize, tail: usize) -> String {
    let mut out = lines(0..n);
    for i in 0..count {
        out.push_str(&format!("agent@2026-10-01T10:{minute:02}:{i:02}.000Z answer {minute}.{i}\n"));
    }
    out + &lines(n..n + tail)
}

fn first_unread_query(after: &str, marker: Option<(usize, String)>) -> FirstUnreadQuery {
    let (since, fp) = marker.map_or((None, None), |(since, fp)| (Some(since as u32), Some(fp)));
    FirstUnreadQuery {
        after: Some(after.to_string()),
        since,
        fp,
    }
}

/// The first unread message is inside the window the client holds and the
/// parse proves: the answer is read off that parse, and it is the answer a
/// fresh read gives — with nothing new to count in the tail, `count` too.
#[tokio::test]
async fn the_first_unread_inside_the_parse_held_is_answered_from_it() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, drive) = drive();
    let id = "first-unread-held";
    // Read up to minute 1; the two answers of minute 2 are unread.
    let file = fake::session(&drive.paths, id, &(with_answers(10, 1, 2, 5) + &with_answers(20, 2, 2, 30)));
    let (held_sig, held) = parse_now(&drive, &file).await;
    let marker = (held.len(), held.prefix_fp());
    append(&file, &lines(100..120));
    let read = "2026-10-01T10:01:59.000Z";

    let found = first_unread_of(&drive, fake::SOURCE, id, &first_unread_query(read, Some(marker))).await.expect("readable");
    assert_eq!(cached_sig(&drive, &file), Some(held_sig), "nothing was parsed");
    let index = found.index.0.expect("an unread message");
    assert_eq!((held.items[index].text.as_str(), found.count), ("answer 2.0", 2));

    let fresh = first_unread_of(&drive, fake::SOURCE, id, &first_unread_query(read, None)).await.expect("readable");
    assert!(read_again(&drive, &file).await, "this one read the file");
    assert_eq!(found, fresh, "the same answer");
}

/// An answer written after the parse held: the index is still the one a
/// fresh read gives, and `count` counts to the end of that parse — the
/// client's whole window — not the message the feed has yet to send it.
#[tokio::test]
async fn the_count_off_the_parse_held_ends_where_it_does() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, drive) = drive();
    let id = "first-unread-count";
    let file = fake::session(&drive.paths, id, &with_answers(10, 2, 2, 30));
    let (_, held) = parse_now(&drive, &file).await;
    let marker = (held.len(), held.prefix_fp());
    append(&file, "agent@2026-10-01T10:05:00.000Z a later answer\n");
    let read = "2026-10-01T10:01:00.000Z";

    let found = first_unread_of(&drive, fake::SOURCE, id, &first_unread_query(read, Some(marker))).await.expect("readable");
    droppen_sessions::loader::forget_transcript(&file);
    let fresh = first_unread_of(&drive, fake::SOURCE, id, &first_unread_query(read, None)).await.expect("readable");
    assert_eq!(found.index, fresh.index);
    assert_eq!((found.count, fresh.count), (2, 3));
}

/// Whenever the parse held cannot prove the answer, the transcript is read
/// as it is now: unread messages only in the tail written since, a first
/// unread among the last items the marker leaves out, a marker the parse
/// does not hold, a client's window longer than the parse, a file
/// rewritten — or no marker at all.
#[tokio::test]
async fn the_first_unread_the_parse_held_cannot_prove_is_read_fresh() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, drive) = drive();
    let id = "first-unread-fresh";
    let file = fake::session(&drive.paths, id, &with_answers(10, 1, 1, 30));
    let read = "2026-10-01T10:01:59.000Z";
    let ask = |query: FirstUnreadQuery| {
        let drive = drive.clone();
        async move { first_unread_of(&drive, fake::SOURCE, id, &query).await.expect("readable") }
    };

    // Nothing unread in the parse; a new answer in the tail.
    let (_, held) = parse_now(&drive, &file).await;
    append(&file, "agent@2026-10-01T10:05:00.000Z new\n");
    let found = ask(first_unread_query(read, Some((held.len(), held.prefix_fp())))).await;
    assert_eq!((found.index.0, found.count), (Some(held.len()), 1), "the message in the tail");
    assert!(read_again(&drive, &file).await);

    // The first unread among the last three items of the window: outside what the marker proves.
    let (_, held) = parse_now(&drive, &file).await;
    append(&file, &lines(200..201));
    let found = ask(first_unread_query(read, Some((held.len(), held.prefix_fp())))).await;
    assert_eq!(found.index.0, Some(held.len() - 1));
    assert!(read_again(&drive, &file).await);

    // A marker the parse does not hold.
    let (_, held) = parse_now(&drive, &file).await;
    append(&file, &lines(201..202));
    ask(first_unread_query(read, Some((held.len(), "not-this-one".into())))).await;
    assert!(read_again(&drive, &file).await);

    // A window one item longer than the parse, whose marker the parse does hold.
    let (_, held) = parse_now(&drive, &file).await;
    append(&file, &lines(202..203));
    let marker = held.fp_at(held.len() + 1 - droppen_sessions::loader::RESUME_OVERLAP).expect("in range");
    ask(first_unread_query(read, Some((held.len() + 1, marker)))).await;
    assert!(read_again(&drive, &file).await);

    // Rewritten shorter: the signature refuses the parse held.
    let (_, held) = parse_now(&drive, &file).await;
    std::fs::write(&file, with_answers(1, 3, 1, 5)).expect("rewrite");
    let found = ask(first_unread_query(read, Some((held.len(), held.prefix_fp())))).await;
    assert_eq!((found.index.0, found.count), (Some(1), 1), "the rewritten file's answer");
    assert!(read_again(&drive, &file).await);

    // No marker: as before.
    parse_now(&drive, &file).await;
    append(&file, &lines(300..301));
    ask(first_unread_query(read, None)).await;
    assert!(read_again(&drive, &file).await);
}

/// The file last written `secs` from now: what it looks like when the
/// agent went on writing for that long after the parse held.
fn written_later(file: &std::path::Path, secs: u64) {
    modified_at(file, std::time::SystemTime::now() + std::time::Duration::from_secs(secs));
}

/// Set the file's mtime to `at`, as `cp -p` or a coarse clock leaves it.
fn modified_at(file: &std::path::Path, at: std::time::SystemTime) {
    std::fs::File::options().write(true).open(file).expect("open").set_modified(at).expect("mtime");
}

/// A reader back at a session whose socket closed while the agent went on
/// writing: the window it kept still matches the last visit's parse, which
/// is all the cache holds, but that parse is a minute behind the file. The
/// first unread is read fresh, and counts every answer written meanwhile.
#[tokio::test]
async fn a_parse_far_behind_the_file_is_not_counted_from() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, drive) = drive();
    let id = "first-unread-returning";
    let file = fake::session(&drive.paths, id, &with_answers(10, 2, 2, 30));
    let (_, held) = parse_now(&drive, &file).await;
    let marker = (held.len(), held.prefix_fp());
    let meanwhile: String = (0..50).map(|i| format!("agent@2026-10-01T11:{:02}:{:02}.000Z later {i}\n", i / 60, i % 60)).collect();
    append(&file, &meanwhile);
    written_later(&file, 60);

    let found = first_unread_of(&drive, fake::SOURCE, id, &first_unread_query("2026-10-01T10:01:00.000Z", Some(marker))).await.expect("readable");
    assert!(read_again(&drive, &file).await, "read fresh");
    assert_eq!((found.index.0, found.count), (Some(10), 52), "the two answers held, and the fifty written since");
}

/// A transcript rewritten longer, five lines put in ahead of everything,
/// before anything parsed it again. A minute on, the parse held is too far
/// behind to answer from, and the answer is the rewritten file's. Within
/// the lag a watched session's feed has, it is not refused: the answer is
/// the one in the numbering of the window the client holds, which the feed
/// is about to replace — the gap the bound leaves.
#[tokio::test]
async fn a_transcript_rewritten_longer_is_answered_fresh_once_past_the_lag() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, drive) = drive();
    let id = "first-unread-rewritten-longer";
    let file = fake::session(&drive.paths, id, &with_answers(10, 2, 2, 30));
    let (_, held) = parse_now(&drive, &file).await;
    let marker = (held.len(), held.prefix_fp());
    let query = first_unread_query("2026-10-01T10:01:00.000Z", Some(marker));
    std::fs::write(&file, (0..5).map(|i| format!("ahead {i}\n")).collect::<String>() + &with_answers(10, 2, 2, 30)).expect("rewrite");

    let within = first_unread_of(&drive, fake::SOURCE, id, &query).await.expect("readable");
    assert_eq!(within.index.0, Some(10), "the held window's numbering");
    assert!(!read_again(&drive, &file).await);

    written_later(&file, 60);
    let past = first_unread_of(&drive, fake::SOURCE, id, &query).await.expect("readable");
    assert_eq!((past.index.0, past.count), (Some(15), 2), "the rewritten file's numbering");
    assert!(read_again(&drive, &file).await);
}

/// An item the client holds rewritten in place, to the same length, then
/// the file appended to, before anything parsed it again: the page is cut
/// from the parse held, as the client's window has it, where a fresh read
/// refuses it. Intended: the feed's next parse replaces that window from
/// the changed item, which drops the page with it.
#[tokio::test]
async fn an_item_rewritten_in_place_before_any_parse_is_paged_as_the_client_holds_it() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, state) = state();
    let id = "history-rewritten-in-place";
    let file = fake::session(&state.paths, id, &lines(0..300));
    let (held_sig, held) = parse_now(&state.drive, &file).await;
    let start_fp = held.fp_at(100).expect("in range");
    let text = std::fs::read_to_string(&file).expect("read");
    std::fs::write(&file, text.replacen("line 50\n", "LINE 50\n", 1)).expect("rewrite in place");
    append(&file, &lines(300..310));

    let page = history(&state, id, 100, Some(start_fp.clone()), None).await.expect("a page");
    assert_eq!((page.sig.as_str(), page.items[50].text.as_str()), (held_sig.as_str(), "line 50"));

    droppen_sessions::loader::forget_transcript(&file);
    let error = history(&state, id, 100, Some(start_fp), None).await.expect_err("refused");
    assert_eq!(error.status(), gotcha::axum::http::StatusCode::CONFLICT);
}

/// The file grew between the route reading its signature and looking in
/// the cache, and a parse of the grown file landed meanwhile: that parse
/// is used, not refused as a rewrite — refused, the route would parse a
/// signature already gone and push the newer parse out.
#[tokio::test]
async fn a_parse_newer_than_the_routes_own_signature_is_used() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, drive) = drive();
    let file = fake::session(&drive.paths, "a-newer-parse", &lines(0..300));
    let read_first = file_signature(&file).await.expect("sig");
    append(&file, &lines(300..310));
    let (newer_sig, newer) = parse_now(&drive, &file).await;

    let mut lag = None;
    let (sig, transcript) = feed::newest_parse_holding(&drive.transcripts, &file, &read_first, |behind, _| {
        lag = Some(behind);
        true
    })
    .await
    .expect("used");
    assert_eq!(sig, newer_sig);
    assert!(Arc::ptr_eq(&transcript, &newer));
    assert_eq!(lag, Some(droppen_sessions::Growth { bytes: 0, millis: 0.0 }), "measured against the file as it is now");
}

/// A transcript replaced by a shorter copy with the same mtime (`cp -p`, or
/// a rewrite within one tick of a coarse clock), nothing parsed since: its
/// signature is behind the parse held, which two signatures cannot tell from
/// a file that grew. Looked at again, the file is not that parse's grown, so
/// the page is read fresh and refused, as on a fresh read.
#[tokio::test]
async fn a_shorter_copy_with_the_same_mtime_is_read_again_and_its_page_refused() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, state) = state();
    let id = "history-shorter-same-mtime";
    let file = fake::session(&state.paths, id, &lines(0..300));
    let (_, held) = parse_now(&state.drive, &file).await;
    let start_fp = held.fp_at(100).expect("in range");
    let mtime = std::fs::metadata(&file).expect("stat").modified().expect("mtime");
    std::fs::write(&file, (0..250).map(|i| format!("other {i}\n")).collect::<String>()).expect("rewrite");
    modified_at(&file, mtime);

    let error = history(&state, id, 100, Some(start_fp), None).await.expect_err("refused");
    assert_eq!(error.status(), gotcha::axum::http::StatusCode::CONFLICT);
    assert!(read_again(&state.drive, &file).await);
}

/// The same with an older copy put back an hour behind, for first unread:
/// read fresh, in the copy's numbering, not off the parse held.
#[tokio::test]
async fn a_shorter_older_copy_is_answered_fresh() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, drive) = drive();
    let id = "first-unread-older-copy";
    let file = fake::session(&drive.paths, id, &((0..5).map(|i| format!("ahead {i}\n")).collect::<String>() + &with_answers(10, 2, 2, 30)));
    let (_, held) = parse_now(&drive, &file).await;
    let marker = (held.len(), held.prefix_fp());
    std::fs::write(&file, with_answers(10, 2, 2, 30)).expect("put the copy back");
    modified_at(&file, std::time::SystemTime::now() - std::time::Duration::from_secs(3600));

    let found = first_unread_of(&drive, fake::SOURCE, id, &first_unread_query("2026-10-01T10:01:00.000Z", Some(marker))).await.expect("readable");
    assert_eq!((found.index.0, found.count), (Some(10), 2), "the copy's numbering");
    assert!(read_again(&drive, &file).await);
}

/// More text gained over the parse than a watched session's feed lags by,
/// all of it within the time bound: read fresh too.
#[tokio::test]
async fn a_parse_too_many_bytes_behind_is_not_counted_from() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, drive) = drive();
    let id = "first-unread-bytes-behind";
    let file = fake::session(&drive.paths, id, &with_answers(10, 2, 2, 30));
    let (held_sig, held) = parse_now(&drive, &file).await;
    let marker = (held.len(), held.prefix_fp());
    append(&file, &("x".repeat(FIRST_UNREAD_MAX_LAG_BYTES as usize) + "\nagent@2026-10-01T11:00:00.000Z later\n"));
    let lag = droppen_sessions::growth(&held_sig, &file_signature(&file).await.expect("sig")).expect("appended");
    assert!(lag.bytes > FIRST_UNREAD_MAX_LAG_BYTES && lag.millis <= FIRST_UNREAD_MAX_LAG_MS, "{lag:?}");

    let found = first_unread_of(&drive, fake::SOURCE, id, &first_unread_query("2026-10-01T10:01:00.000Z", Some(marker))).await.expect("readable");
    assert!(read_again(&drive, &file).await, "read fresh");
    assert_eq!((found.index.0, found.count), (Some(10), 3));
}
