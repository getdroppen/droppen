//! `claude setup-token`, captured off a pty.
//!
//! Port of the former TypeScript server's `setup-token.ts` plus the two routes
//! that drive it in its `app.ts`.
//!
//! `claude setup-token` is an OAuth dance: the CLI prints an authorization URL,
//! the user authorizes it and pastes the code back, and the CLI finally prints a
//! long-lived token. There is no machine-readable output and no file to read, so
//! the token is taken the only way it is offered — off the terminal's own byte
//! stream — verified against Anthropic, and handed to the accounts store.
//!
//! The token it mints carries the `user:inference` scope only. It can run
//! sessions but cannot read usage; that needs the separate `claude auth login`
//! in [`super::usage_login`].
//!
//! # Nothing opens a browser
//!
//! The CLI's first move is to run the platform's browser opener (`open` on
//! macOS, `xdg-open` elsewhere) on a URL whose redirect goes to a `localhost`
//! port *on this server's machine*. For a remote-hand reached from a phone that
//! is not merely useless, it is a window opening on somebody else's desktop. So
//! the wizard's `PATH` is prefixed with a directory of no-op openers
//! ([`stub_browser_openers`]) and its `BROWSER` points at one of them: the
//! attempt succeeds, silently, and does nothing.
//!
//! The CLI then falls back to printing the URL it *meant* to open — the
//! `code=true` variant, whose redirect shows the user a code to copy. That URL
//! is scraped off the stream ([`find_auth_url`]) and reported to the browser,
//! which shows it as copyable text; the code comes back through
//! [`submit_setup_token_code`] and is typed into the pty.
//!
//! The one exception is the desktop app asking its own server from this very
//! device ([`crate::setup::app_caller`]):
//! there the browser the CLI opens *is* the user's, so the stubs are left out
//! and the authorization page simply opens. The URL is still scraped and shown,
//! as the fallback for a browser that did not come up.
//!
//! # Wrapped output
//!
//! The CLI hard-wraps at the terminal width — which used to be the pty's width,
//! which the attached browser set to whatever its pane happened to be. A
//! ~108-character token printed into a 101-column terminal arrives as two lines,
//! and a scanner that stops at the first line stores 101 characters of a token:
//! a credential that looks entirely plausible and 401s on every request.
//!
//! Three things now stand between that and the store, in order of how much they
//! are relied on. The pty's width is pinned wide ([`WIZARD_SIZE`]) and no client
//! can narrow it, so nothing the wizard prints reaches a margin. The scanners
//! rejoin runs across a line break anyway ([`wrapped_segments`]), for a CLI that
//! wraps to something narrower than its terminal. And because a rejoin cannot be
//! told from a hint printed on the next line, the token scanner reports *both*
//! readings and [`super::verify`] puts the question to Anthropic rather than
//! guessing.
//!
//! # Shape
//!
//! The TypeScript queues the capture and the exit cleanup behind one promise
//! chain, because both mutate the same record from the node event loop. Here the
//! two pty hooks are plain synchronous callbacks that can fire from different
//! threads, so they only *report*: each sends an [`Event`] down an unbounded
//! channel, and one task per capture ([`run_capture`]) consumes it in order and
//! owns every mutation. That is the same serialization, expressed as ownership
//! rather than as a promise chain.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use gotcha::{api, Schematic, State};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::api::created::Created;
use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath};
use crate::api::{AppState, SendResult};
use crate::drive::Drive;
use crate::openapi_ext::Optional;
use crate::process_env::{claude_env, clean_env_bare, resolve_on_path};
use crate::terminals::{CreateSpec, Registry};

use super::store::{AccountError, AccountKind, AccountPatch, AccountProvider, Credential};
use super::verify::{verify_secret, Verdict};

/// How long a finished capture stays pollable.
const EXPIRY: Duration = Duration::from_secs(60 * 60);
/// Long enough for the CLI to print its closing hint and exit by itself.
const CAPTURE_GRACE: Duration = Duration::from_secs(1);
/// The wizard prints a screen or two, not a build log.
const SCAN_CAP: usize = 64 * 1024;

// ------------------------------------------------------------------ scanning

/// The credential prefix `claude setup-token` mints.
const TOKEN_PREFIX: &str = "sk-ant-oat01-";
/// The shortest body that could be a whole token.
const MIN_TOKEN_BODY: usize = 60;
/// The longest one worth believing. A join that runs past this has swallowed
/// something that is not token.
const MAX_TOKEN_BODY: usize = 400;
/// How many terminal line wraps one value may be spread over.
///
/// Six covers a token or an authorization URL on a phone-width pane; beyond that
/// the run being followed is prose, not a wrapped value.
const MAX_WRAPS: usize = 6;

/// The wizard's pty size: opened this wide, and not narrowable by any client.
///
/// This is the *first* of the defences against a wrapped token, and the one that
/// stops the problem happening: the CLI hard-wraps at the terminal width, and at
/// 400 columns neither the token (~110 characters) nor the authorization URL
/// (~330) reaches the margin. The rejoining below is the second defence, for a
/// CLI that wraps to something narrower than its terminal.
const WIZARD_SIZE: (u16, u16) = (400, 40);

/// Every escape is replaced by a space, never deleted.
///
/// Deleting once spliced the trailing "Store this token…" hint onto the token
/// itself and produced a silently unusable account, so the separator has to
/// survive the strip.
fn ansi_re() -> &'static regex::Regex {
    static ANSI: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    ANSI.get_or_init(|| {
        regex::Regex::new(concat!(
            r"[\x1b\u{9b}]",
            r"(?:",
            r"\[[0-?]*[ -/]*[@-~]",                  // CSI
            r"|\][^\x07\x1b]{0,512}(?:\x07|\x1b\\)", // OSC, terminated by BEL or ST
            r"|[@-Z\\-_]",                           // Fe, two bytes
            r")",
        ))
        .expect("valid ANSI pattern")
    })
}

/// Replace every ANSI escape sequence with a single space.
pub fn strip_ansi(text: &str) -> String {
    ansi_re().replace_all(text, " ").into_owned()
}

/// A character a token body is made of.
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// A character a printed URL is made of.
fn is_url_char(c: char) -> bool {
    c.is_ascii_graphic()
}

/// What a terminal line wrap is allowed to be made of.
///
/// Carriage returns come in pairs on a pty (`\r\r\n`), and an escape stripped to
/// a space sits on either side of the break, so the break is "whitespace with
/// exactly one newline in it" rather than a bare `\n`.
fn is_gap_char(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

/// The run of `matches` characters `text` starts with.
fn run_of(text: &str, matches: fn(char) -> bool) -> &str {
    let end = text.find(|c: char| !matches(c)).unwrap_or(text.len());
    &text[..end]
}

/// How long the line wrap at the start of `text` is, or `None` if there is none.
///
/// A *blank* line is not a wrap: a second newline ends the run, which is what
/// keeps a paragraph of prose after the value from being joined onto it.
fn wrap_gap(text: &str) -> Option<usize> {
    let mut used = 0;
    let mut newline = false;
    for c in text.chars() {
        match c {
            '\n' if !newline => {
                newline = true;
                used += 1;
            }
            c if is_gap_char(c) && c != '\n' => used += c.len_utf8(),
            _ => break,
        }
    }
    newline.then_some(used)
}

/// Whether everything left is whitespace — i.e. whether a continuation could
/// still be on its way.
fn only_gap(text: &str) -> bool {
    text.chars().all(is_gap_char)
}

/// What one value looks like when the terminal has wrapped it.
#[derive(Debug, PartialEq, Eq)]
enum Segments<'a> {
    /// The runs the value is spread over, first one first.
    Runs(Vec<&'a str>),
    /// The stream stopped mid-value; nothing can be concluded yet.
    Incomplete,
}

/// Read a token body out of `text` at `at`, following it across line wraps.
///
/// `settled` says the stream is over, so a run that reaches the end of the text
/// is whole rather than still arriving.
///
/// A break followed by more token characters is *taken* as a wrap, even though
/// prose on the next line joins identically. That guess is only safe because it
/// is never acted on alone: both readings come back from [`readings`] and
/// [`choose_reading`] makes Anthropic decide. Nothing else in this module may
/// join a wrap on the same terms — see [`printed_auth_url`].
fn wrapped_segments(text: &str, at: usize, settled: bool) -> Segments<'_> {
    let mut cursor = at;
    let mut runs = Vec::new();
    loop {
        let run = run_of(&text[cursor..], is_token_char);
        cursor += run.len();
        if !run.is_empty() {
            runs.push(run);
        }
        // Anything still arriving would extend this run, or be its continuation.
        if !settled && only_gap(&text[cursor..]) {
            return Segments::Incomplete;
        }
        if runs.len() > MAX_WRAPS {
            break;
        }
        let Some(gap) = wrap_gap(&text[cursor..]) else { break };
        let after = cursor + gap;
        if run_of(&text[after..], is_token_char).is_empty() {
            break;
        }
        cursor = after;
    }
    Segments::Runs(runs)
}

/// One captured token, as every reading of it the stream allows.
///
/// Two readings, when a line break falls inside the run: the line the token
/// started on, and the run rejoined across the break. They cannot be told apart
/// from the bytes alone — a hint printed at column 0 on the next line joins
/// exactly as a wrapped continuation would — so both are offered, in the order
/// "what the screen literally says" first, and the live check in
/// [`super::verify`] decides between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenMatch {
    readings: Vec<String>,
}

impl TokenMatch {
    /// Every reading, likeliest first.
    pub fn candidates(&self) -> &[String] {
        &self.readings
    }

    /// What the screen literally showed: the token on the line it started on.
    ///
    /// Not a recommendation, and deliberately not called one. When the terminal
    /// wrapped the token this is the *truncated* reading, and storing it is the
    /// 401 this module exists to prevent — which is why the only thing allowed
    /// to pick between readings is [`choose_reading`].
    pub fn printed(&self) -> &str {
        self.readings.first().map(String::as_str).unwrap_or_default()
    }
}

/// Accumulates raw pty output and reports what the wizard has printed.
///
/// Raw, not pre-cleaned: an escape sequence split across two chunks would leave
/// garbage in a per-chunk strip, and that garbage could land inside the token.
#[derive(Debug, Default)]
pub struct TokenScanner {
    buffer: String,
}

impl TokenScanner {
    /// Feed a chunk; returns the token once it is whole, or `None` before that.
    pub fn push(&mut self, chunk: &str) -> Option<TokenMatch> {
        self.buffer.push_str(chunk);
        if self.buffer.len() > SCAN_CAP {
            // `String::slice(-SCAN_CAP)` in the TypeScript; here the cut has to
            // land on a character boundary, and erring towards keeping *more*
            // than the cap can never split a token that is still arriving.
            let mut cut = self.buffer.len() - SCAN_CAP;
            while cut > 0 && !self.buffer.is_char_boundary(cut) {
                cut -= 1;
            }
            self.buffer.drain(..cut);
        }
        self.scan(false)
    }

    /// Last look once the stream is over, for a token with nothing printed after it.
    pub fn flush(&mut self) -> Option<TokenMatch> {
        self.scan(true)
    }

    /// The authorization URL the CLI printed, once it has printed one.
    pub fn auth_url(&self) -> Option<String> {
        find_auth_url(&self.buffer)
    }

    fn scan(&self, stream_ended: bool) -> Option<TokenMatch> {
        let cleaned = strip_ansi(&self.buffer);
        for (start, _) in cleaned.match_indices(TOKEN_PREFIX) {
            let body = start + TOKEN_PREFIX.len();
            let runs = match wrapped_segments(&cleaned, body, stream_ended) {
                // The token is still being printed. Nothing later in the buffer
                // can be a *finished* token either, so the whole scan waits.
                Segments::Incomplete => return None,
                Segments::Runs(runs) => runs,
            };
            if let Some(found) = readings(&runs) {
                return Some(found);
            }
        }
        None
    }
}

/// Turn the runs a token was spread over into the readings worth keeping.
fn readings(runs: &[&str]) -> Option<TokenMatch> {
    let joined = runs.concat();
    let mut readings = Vec::new();
    let mut offer = |body: &str| {
        // A second `sk-ant-oat01-` inside the body means the join ran off the end
        // of one token and onto the next line's; that is never a token.
        if (MIN_TOKEN_BODY..=MAX_TOKEN_BODY).contains(&body.len()) && !body.contains(TOKEN_PREFIX) {
            let reading = format!("{TOKEN_PREFIX}{body}");
            if !readings.contains(&reading) {
                readings.push(reading);
            }
        }
    };
    offer(runs.first().copied().unwrap_or_default());
    if runs.len() > 1 {
        offer(&joined);
    }
    (!readings.is_empty()).then_some(TokenMatch { readings })
}

/// The OAuth URL the CLI printed, taken from the hyperlink it printed it as.
///
/// The visible text is wrapped to the terminal width, but a terminal hyperlink
/// (OSC 8) carries the whole URL in its parameter, unwrapped and delimited — so
/// that is read first, and the wrapped text is only rejoined when the CLI did
/// not emit a hyperlink at all.
pub fn find_auth_url(raw: &str) -> Option<String> {
    osc8_auth_url(raw).or_else(|| printed_auth_url(&strip_ansi(raw)))
}

/// The URL as it stands on one line of the screen, or nothing.
///
/// Deliberately unable to rejoin a wrap. The token scanner may guess and offer
/// both readings because a live check settles which is right; a URL has no such
/// arbiter — a wrong one is handed to the user as the thing to go and authorize.
/// And the guess would be wrong in the shape that actually occurs: the CLI
/// prints its "Paste code here if prompted" line directly under the URL, whose
/// first word joins across the break exactly as a continuation would.
///
/// So a run that is followed by more URL characters on the next line is reported
/// as nothing at all, rather than as either half of itself. In practice this
/// never fires: the CLI emits the whole URL inside a terminal hyperlink, and the
/// wizard's pty is pinned far wider ([`WIZARD_SIZE`]) than the URL is long.
fn printed_auth_url(cleaned: &str) -> Option<String> {
    for (start, _) in cleaned.match_indices("https://") {
        let run = run_of(&cleaned[start..], is_url_char);
        let rest = &cleaned[start + run.len()..];
        // Still arriving: the tail could be more URL.
        if only_gap(rest) {
            continue;
        }
        // Wrapped — or followed by prose that would join as if it were wrapped.
        // Either way this line is not a whole URL and cannot be made into one.
        if let Some(gap) = wrap_gap(rest) {
            if !run_of(&rest[gap..], is_url_char).is_empty() {
                continue;
            }
        }
        if is_auth_url(run) {
            return Some(run.to_string());
        }
    }
    None
}

/// `ESC ] 8 ; <params> ; <uri> BEL` — or `ST` instead of `BEL`.
fn osc8_auth_url(raw: &str) -> Option<String> {
    let mut rest = raw;
    while let Some(at) = rest.find("\u{1b}]8;") {
        let after = &rest[at + "\u{1b}]8;".len()..];
        let semicolon = after.find(';')?;
        let body = &after[semicolon + 1..];
        // Unterminated: the rest of the sequence has not arrived yet, and
        // half a URL is worse than none.
        let end = body.find('\u{7}').or_else(|| body.find("\u{1b}\\"))?;
        let url = &body[..end];
        if is_auth_url(url) {
            return Some(url.to_string());
        }
        rest = &body[end..];
    }
    None
}

/// Whether this is the URL the user is meant to authorize.
fn is_auth_url(url: &str) -> bool {
    url.starts_with("https://") && url.contains("/oauth/authorize")
}

// ----------------------------------------------------------- browser openers

/// Every name a "please open this URL" call might reach for.
///
/// A `PATH` shadow, so it covers a call by *name* — which is what every
/// launcher library actually does. It does not cover an absolute path
/// (`/usr/bin/open`) or a direct syscall, and cannot: those would need a
/// sandbox, not an environment variable. That is an accepted limit. The CLI
/// today calls the bare name, and if a future one stops doing so the fallback it
/// already prints — the URL, on screen — is what this flow reads anyway.
const BROWSER_OPENERS: [&str; 18] = [
    "open",
    "xdg-open",
    "gnome-open",
    "kde-open",
    "gio",
    "wslview",
    "termux-open-url",
    "x-www-browser",
    "www-browser",
    "sensible-browser",
    "firefox",
    "google-chrome",
    "chrome",
    "chromium",
    "chromium-browser",
    "microsoft-edge",
    "brave",
    "opera",
];

/// Fill `dir` with openers that do nothing, and return the `PATH`/`BROWSER`
/// entries that make the wizard find them first.
///
/// A stub per *name* rather than one environment variable, because there is no
/// one variable: the CLI shells out to the platform opener directly, and
/// `BROWSER` is only honoured by some of the libraries that might be underneath.
/// Shadowing the names on `PATH` covers both, and covers whatever the next
/// version reaches for.
async fn stub_browser_openers(dir: &Path) -> std::io::Result<Vec<(String, String)>> {
    tokio::fs::create_dir_all(dir).await?;
    for name in BROWSER_OPENERS {
        let path = dir.join(name);
        tokio::fs::write(&path, "#!/bin/sh\nexit 0\n").await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).await?;
        }
    }
    // The children's PATH, not the inherited one: a CLI the guide found off
    // the PATH ([`crate::setup`]) must still be found behind the stubs.
    let inherited = crate::process_env::child_path();
    Ok(vec![
        ("PATH".to_string(), format!("{}:{inherited}", dir.display())),
        ("BROWSER".to_string(), dir.join("open").to_string_lossy().into_owned()),
    ])
}

// ----------------------------------------------------------------- wire shapes

/// Where a capture stands.
///
/// The name is the inline enum in `openapi/openapi.json`'s `SetupTokenStatus`;
/// Rust needs a named type for it, which the contract reads as the same string
/// union.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum SetupTokenState {
    /// The wizard is running and no token has appeared yet.
    Pending,
    /// A token was captured and stored; `accountId` names the account.
    Captured,
    /// The CLI exited without ever printing a token.
    Exited,
    /// A token appeared but could not be stored.
    Error,
}

/// `POST /api/accounts/setup-token` body.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct SetupTokenStart {
    /// Display name for the account the wizard will create.
    pub label: String,
}

/// `POST /api/accounts/setup-token/{captureId}/code` body.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct SetupTokenCode {
    /// The authorization code the OAuth page showed, as the user copied it.
    pub code: String,
}

/// `POST /api/accounts/setup-token` response.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SetupTokenStarted {
    /// Poll `GET /api/accounts/setup-token/{captureId}` with this.
    pub capture_id: String,
    /// Attach this terminal to watch the wizard.
    pub terminal_id: String,
}

/// What a status poll is allowed to see: an account id, never the token.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SetupTokenStatus {
    /// Where the capture stands.
    pub status: SetupTokenState,
    /// The terminal the wizard is running in.
    pub terminal_id: String,
    /// The URL to authorize, once the CLI has printed one. Nothing opens it for
    /// the user: it is shown as text to copy.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub auth_url: Optional<String>,
    /// Present once captured; the token itself never leaves the server.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub account_id: Optional<String>,
    /// Why the captured token was stored without being checked against
    /// Anthropic. Absent when a check ran and accepted it.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub unverified: Optional<String>,
    /// How the CLI exited, when it exited without a token.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub exit_code: Optional<i32>,
    /// The token was already on file: `accountId` names the existing account.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub reused: Optional<bool>,
    /// Why the token could not be stored.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub error: Optional<String>,
}

/// Why the wizard could not be started.
#[derive(Debug, thiserror::Error)]
pub enum SetupTokenError {
    /// `claude` is not installed, or not on this server's `PATH`.
    #[error("the `claude` CLI is not on PATH, so the token wizard cannot start")]
    NoCli,
    /// The throwaway config directory could not be made.
    #[error("cannot prepare the token wizard's config directory: {0}")]
    Scratch(String),
    /// The pty could not be opened, or `claude` could not be spawned.
    #[error("cannot start the token wizard")]
    Terminal,
}

/// What a finished capture does with the token it caught.
#[derive(Debug, Clone)]
pub enum Target {
    /// File it as a new account under this label.
    New(String),
    /// Replace this account's credential in place, keeping its id — and with the
    /// id, every session already bound to it.
    Existing {
        /// The account being re-authorized.
        id: String,
        /// Its label, for the terminal's title.
        label: String,
    },
}

impl Target {
    fn label(&self) -> &str {
        match self {
            Target::New(label) => label,
            Target::Existing { label, .. } => label,
        }
    }
}

// -------------------------------------------------------------- the registry

/// One capture, as the server tracks it.
#[derive(Debug)]
struct Entry {
    status: SetupTokenState,
    terminal_id: String,
    auth_url: Option<String>,
    account_id: Option<String>,
    unverified: Option<String>,
    exit_code: Option<i32>,
    reused: bool,
    error: Option<String>,
    /// The throwaway `CLAUDE_CONFIG_DIR` tree, removed when the wizard exits.
    home: PathBuf,
}

impl Entry {
    fn to_status(&self) -> SetupTokenStatus {
        SetupTokenStatus {
            status: self.status,
            terminal_id: self.terminal_id.clone(),
            auth_url: Optional(self.auth_url.clone()),
            account_id: Optional(self.account_id.clone()),
            unverified: Optional(self.unverified.clone()),
            exit_code: Optional(self.exit_code),
            // `if (entry.reused) status.reused = true` — never a literal `false`.
            reused: Optional(self.reused.then_some(true)),
            error: Optional(self.error.clone()),
        }
    }
}

/// Every capture this server has started, live and recently finished.
///
/// Lives in [`Drive`] rather than in a `static` — like every other registry in
/// the port, so two servers in one process (which is what the integration tests
/// run) cannot see each other's captures.
#[derive(Debug, Default)]
pub struct Captures {
    entries: Mutex<HashMap<String, Entry>>,
}

impl Captures {
    /// An empty registry, ready to be shared by every handler.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// What a status poll sees, or `None` for an unknown or expired capture.
    pub fn status(&self, capture_id: &str) -> Option<SetupTokenStatus> {
        self.lock().get(capture_id).map(Entry::to_status)
    }

    /// How many captures are on file; the expiry sweep is observable through it.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no capture is on file.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().expect("setup-token captures poisoned")
    }

    fn insert(&self, capture_id: String, terminal_id: String, home: PathBuf) {
        self.lock().insert(
            capture_id,
            Entry {
                status: SetupTokenState::Pending,
                terminal_id,
                auth_url: None,
                account_id: None,
                unverified: None,
                exit_code: None,
                reused: false,
                error: None,
                home,
            },
        );
    }

    fn terminal_id(&self, capture_id: &str) -> Option<String> {
        self.lock().get(capture_id).map(|entry| entry.terminal_id.clone())
    }

    fn home(&self, capture_id: &str) -> Option<PathBuf> {
        self.lock().get(capture_id).map(|entry| entry.home.clone())
    }

    fn saw_auth_url(&self, capture_id: &str, url: String) {
        if let Some(entry) = self.lock().get_mut(capture_id) {
            entry.auth_url = Some(url);
        }
    }

    fn captured(&self, capture_id: &str, account_id: String, reused: bool, unverified: Option<String>) {
        if let Some(entry) = self.lock().get_mut(capture_id) {
            entry.status = SetupTokenState::Captured;
            entry.account_id = Some(account_id);
            entry.reused = reused;
            entry.unverified = unverified;
        }
    }

    fn failed(&self, capture_id: &str, message: String) {
        if let Some(entry) = self.lock().get_mut(capture_id) {
            entry.status = SetupTokenState::Error;
            entry.error = Some(message);
        }
    }

    /// Record the exit, but only over a capture that never got anywhere: a
    /// stored token stays `captured` however the CLI then went away.
    fn note_exit(&self, capture_id: &str, exit_code: i32) {
        if let Some(entry) = self.lock().get_mut(capture_id) {
            if entry.status == SetupTokenState::Pending {
                entry.status = SetupTokenState::Exited;
                entry.exit_code = Some(exit_code);
            }
        }
    }

    fn remove(&self, capture_id: &str) {
        self.lock().remove(capture_id);
    }
}

// ------------------------------------------------------------------ the wizard

/// What a pty hook tells [`run_capture`].
#[derive(Debug)]
enum Event {
    /// The CLI printed the URL the user has to authorize.
    AuthUrl(String),
    /// A whole token was seen on the stream.
    Token(TokenMatch),
    /// The process is gone.
    Exit(i32),
}

/// Start the wizard: a pty running `claude setup-token`, watched for a token.
///
/// `open_browser` leaves the CLI's browser opener alone (see the module docs:
/// only for the desktop app on its own device); otherwise every opener is a
/// no-op.
pub async fn start(drive: Arc<Drive>, terminals: Arc<Registry>, target: Target, open_browser: bool) -> Result<SetupTokenStarted, SetupTokenError> {
    if resolve_on_path("claude", &clean_env_bare()).is_none() {
        return Err(SetupTokenError::NoCli);
    }

    let capture_id = uuid::Uuid::new_v4().to_string();
    // A directory of our own, never ~/.claude and never the managed root: the
    // wizard writes credentials there that we do not want and do not keep.
    let home = drive.paths.data_dir().join("setup-token").join(&capture_id);
    let config_dir = home.join("claude");
    let _ = tokio::fs::remove_dir_all(&home).await;
    tokio::fs::create_dir_all(&config_dir)
        .await
        .map_err(|error| SetupTokenError::Scratch(error.to_string()))?;
    let no_browser = if open_browser {
        Vec::new()
    } else {
        stub_browser_openers(&home.join("bin"))
            .await
            .map_err(|error| SetupTokenError::Scratch(error.to_string()))?
    };

    let (events, incoming) = mpsc::unbounded_channel();
    let scanner = Arc::new(Mutex::new(TokenScanner::default()));
    let claimed = Arc::new(AtomicBool::new(false));
    let announced = Arc::new(AtomicBool::new(false));

    let mut spec = CreateSpec::new("claude", vec!["setup-token".to_string()]);
    spec.env = Some(claude_env(
        [
            ("CLAUDE_CONFIG_DIR".to_string(), config_dir.to_string_lossy().into_owned()),
            ("TERM".to_string(), "xterm-256color".to_string()),
        ]
        .into_iter()
        .chain(no_browser),
    ));
    spec.cwd = Some(home.to_string_lossy().into_owned());
    spec.title = Some(format!("claude setup-token — {}", target.label()));
    spec.size = Some(WIZARD_SIZE);
    spec.on_data = Some(Box::new({
        let (scanner, claimed, announced, events) = (scanner.clone(), claimed.clone(), announced.clone(), events.clone());
        move |chunk: &str| {
            if claimed.load(Ordering::SeqCst) {
                return;
            }
            let mut scanner = scanner.lock().expect("token scanner poisoned");
            let found = scanner.push(chunk);
            if !announced.load(Ordering::SeqCst) {
                if let Some(url) = scanner.auth_url() {
                    if !announced.swap(true, Ordering::SeqCst) {
                        let _ = events.send(Event::AuthUrl(url));
                    }
                }
            }
            drop(scanner);
            let Some(token) = found else { return };
            if !claimed.swap(true, Ordering::SeqCst) {
                let _ = events.send(Event::Token(token));
            }
        }
    }));
    spec.on_exit = Some(Box::new({
        let (scanner, claimed, events) = (scanner, claimed, events.clone());
        move |exit_code: i32| {
            if !claimed.load(Ordering::SeqCst) {
                let found = scanner.lock().expect("token scanner poisoned").flush();
                if let Some(token) = found {
                    if !claimed.swap(true, Ordering::SeqCst) {
                        let _ = events.send(Event::Token(token));
                    }
                }
            }
            let _ = events.send(Event::Exit(exit_code));
        }
    }));

    let info = terminals.create_terminal_with(spec).await.map_err(|error| {
        tracing::error!(%error, "the setup-token wizard could not open a terminal");
        SetupTokenError::Terminal
    })?;
    // The hooks hold the only senders left, so the channel closes — and
    // `run_capture` ends — once the terminal is gone for good.
    drop(events);

    drive.wizards.insert(capture_id.clone(), info.id.clone(), home);
    tokio::spawn(run_capture(drive, terminals, capture_id.clone(), target, incoming));
    Ok(SetupTokenStarted {
        capture_id,
        terminal_id: info.id,
    })
}

/// The one owner of a capture's mutations, fed by the two pty hooks in order.
async fn run_capture(drive: Arc<Drive>, terminals: Arc<Registry>, capture_id: String, target: Target, mut incoming: mpsc::UnboundedReceiver<Event>) {
    while let Some(event) = incoming.recv().await {
        match event {
            Event::AuthUrl(url) => drive.wizards.saw_auth_url(&capture_id, url),
            Event::Token(found) => {
                store(&drive, &capture_id, &target, &found).await;
                retire_terminal(&drive.wizards, &terminals, &capture_id);
            }
            Event::Exit(exit_code) => {
                drive.wizards.note_exit(&capture_id, exit_code);
                if let Some(home) = drive.wizards.home(&capture_id) {
                    let _ = tokio::fs::remove_dir_all(&home).await;
                }
                expire_later(Arc::downgrade(&drive.wizards), capture_id.clone());
            }
        }
    }
}

/// Which reading of the capture Anthropic accepts.
///
/// Split from its caller so the decision can be tested against every order of
/// verdicts without a server, a pty or a network — the arithmetic below is the
/// part that decides whether a 401 gets written to disk.
///
/// Three rules, in the order they matter:
///
/// * a reading Anthropic accepts wins, whatever was said about the others;
/// * a reading Anthropic *refused* is never stored, and never becomes a fallback
///   for a later check that could not run;
/// * when the readings disagree and no check settled which is which, nothing is
///   stored. One of them is a token that will 401 on every request while looking
///   perfectly fine in the account list, and there is no honest way to pick.
///
/// The single-reading case is the exception, and deliberately so: with nothing
/// to choose between, an unrunnable check is the machine's problem rather than
/// the token's, and refusing every account on an offline box would be a worse
/// failure than the one this guards against.
async fn choose_reading<F, Fut>(found: &TokenMatch, check: F) -> Result<Reading, String>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Verdict>,
{
    let ambiguous = found.candidates().len() > 1;
    let mut refused = None;
    for candidate in found.candidates() {
        match check(candidate.clone()).await {
            Verdict::Valid => return Ok(Reading::checked(candidate)),
            Verdict::Invalid(reason) => refused = Some(reason),
            // The check is broken, not the token — and a second reading would
            // break it the same way, so stop rather than spend another probe.
            Verdict::Unverifiable(why) if !ambiguous => {
                tracing::warn!(%why, "storing a captured token without checking it");
                return Ok(Reading::unchecked(candidate, why));
            }
            Verdict::Unverifiable(why) => {
                return Err(format!(
                    "the terminal split the token across lines, so it could be read two ways, \
                     and the check that would say which could not run ({why}). Nothing was stored — run the wizard again."
                ))
            }
        }
    }
    Err(refused.unwrap_or_else(|| "the captured token was refused".to_string()))
}

/// The token to store, and whether anything actually vouched for it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reading {
    token: String,
    /// Why no check ran. `None` means Anthropic accepted this token.
    unverified: Option<String>,
}

impl Reading {
    fn checked(token: &str) -> Self {
        Self {
            token: token.to_string(),
            unverified: None,
        }
    }

    fn unchecked(token: &str, why: String) -> Self {
        Self {
            token: token.to_string(),
            unverified: Some(why),
        }
    }
}

/// [`choose_reading`] against the real Anthropic check.
async fn accepted_reading(drive: &Arc<Drive>, found: &TokenMatch) -> Result<Reading, String> {
    let paths = drive.paths.clone();
    choose_reading(found, move |candidate| {
        let paths = paths.clone();
        async move { verify_secret(&paths, AccountProvider::Claude, &Credential::Oauth(candidate)).await }
    })
    .await
}

/// Hand the captured token to the accounts store, and record where it landed.
async fn store(drive: &Arc<Drive>, capture_id: &str, target: &Target, found: &TokenMatch) {
    let reading = match accepted_reading(drive, found).await {
        Ok(reading) => reading,
        Err(reason) => return drive.wizards.failed(capture_id, reason),
    };
    let unverified = reading.unverified;
    match target {
        Target::New(label) => match drive
            .accounts
            .create(label, AccountProvider::Claude, Credential::Oauth(reading.token.clone()))
            .await
        {
            Ok(account) => drive.wizards.captured(capture_id, account.id, false, unverified),
            // Re-authorizing a login already on file is a success, not a clash. The
            // stored label wins: it is the one the user has been reading in the
            // account list, and this run only proved the token still works.
            Err(AccountError::Duplicate { existing_id, .. }) => {
                let _ = drive.accounts.update(&existing_id, AccountPatch::default()).await;
                drive.wizards.captured(capture_id, existing_id, true, unverified);
            }
            Err(error) => drive.wizards.failed(capture_id, error.to_string()),
        },
        Target::Existing { id, .. } => {
            let patch = AccountPatch {
                credential: Some(Credential::Oauth(reading.token)),
                ..Default::default()
            };
            match drive.accounts.update(id, patch).await {
                Ok(Some(account)) => drive.wizards.captured(capture_id, account.id, false, unverified),
                Ok(None) => drive.wizards.failed(capture_id, "that account is gone".to_string()),
                Err(error) => drive.wizards.failed(capture_id, error.to_string()),
            }
        }
    }
}

/// The token is on screen and in the terminal's replay buffer the instant it is
/// printed, so the buffer goes immediately; the pty itself gets a moment to exit
/// on its own (it prints a closing hint) before being killed.
fn retire_terminal(wizards: &Arc<Captures>, terminals: &Arc<Registry>, capture_id: &str) {
    let Some(terminal_id) = wizards.terminal_id(capture_id) else { return };
    terminals.clear_terminal_replay(&terminal_id);
    let terminals = terminals.clone();
    tokio::spawn(async move {
        tokio::time::sleep(CAPTURE_GRACE).await;
        terminals.clear_terminal_replay(&terminal_id);
        terminals.kill_terminal(&terminal_id);
    });
}

/// Forget a finished capture an hour later, holding nothing alive in the meantime.
fn expire_later(wizards: Weak<Captures>, capture_id: String) {
    tokio::spawn(async move {
        tokio::time::sleep(EXPIRY).await;
        if let Some(wizards) = wizards.upgrade() {
            wizards.remove(&capture_id);
        }
    });
}

// ------------------------------------------------------------------- the routes

/// Start the `claude setup-token` wizard in a terminal.
#[api(id = "start_setup_token", group = "accounts", summary = "Start the setup-token wizard")]
pub async fn start_setup_token(
    State(state): State<AppState>,
    #[api(skip)] caller: crate::setup::Caller,
    ApiJson(body): ApiJson<SetupTokenStart>,
) -> Result<Created<SetupTokenStarted>, ApiError> {
    // `z.string().min(1).max(100)`, which fails in the validator before the
    // TypeScript handler runs — so it is a 400 there too, undocumented.
    super::require_label(&body.label)?;
    let open_browser = crate::setup::app_caller(&state, &caller).is_ok();
    start(state.drive.clone(), state.terminals.clone(), Target::New(body.label), open_browser)
        .await
        .map(Created)
        .map_err(|error| ApiError::internal(error.to_string()))
}

/// Run the wizard again for an account that already exists.
///
/// The account keeps its id, which is the whole point: `sessionAccounts` binds
/// sessions to that id, so a session mid-conversation carries on under the same
/// account with a credential that works again.
#[api(id = "reauth_account", group = "accounts", summary = "Re-authorize an account")]
pub async fn reauth_account(
    State(state): State<AppState>,
    #[api(skip)] caller: crate::setup::Caller,
    ApiPath(id): ApiPath<String>,
) -> Result<Created<SetupTokenStarted>, ApiError> {
    super::require_uuid(&id)?;
    let Some(account) = state.drive.accounts.get(&id).await else {
        return Err(ApiError::not_found("account not found"));
    };
    // `setup-token` mints a Claude OAuth token and nothing else: a pasted key
    // or bearer token is re-entered rather than re-authorized, and codex has no
    // OAuth token remote-hand could inject even if the wizard produced one.
    if account.provider == AccountProvider::Codex {
        return Err(ApiError::conflict(
            "codex accounts hold a local API key, so there is no OAuth wizard to run",
        ));
    }
    if account.kind != AccountKind::OauthToken {
        return Err(ApiError::conflict("setup_token_not_supported"));
    }
    let target = Target::Existing {
        id: account.id,
        label: account.label,
    };
    let open_browser = crate::setup::app_caller(&state, &caller).is_ok();
    start(state.drive.clone(), state.terminals.clone(), target, open_browser)
        .await
        .map(Created)
        .map_err(|error| ApiError::internal(error.to_string()))
}

/// Type the authorization code into the waiting wizard.
///
/// The browser never sees the pty; it hands the code here and the server types
/// it, so the flow works from a phone that cannot reach this machine's terminal.
#[api(id = "submit_setup_token_code", group = "accounts", summary = "Submit the OAuth code to a capture")]
pub async fn submit_setup_token_code(
    State(state): State<AppState>,
    ApiPath(capture_id): ApiPath<String>,
    ApiJson(body): ApiJson<SetupTokenCode>,
) -> Result<gotcha::Json<SendResult>, ApiError> {
    let code = body.code.trim();
    // One line of one paste. Anything with a control character in it is either a
    // mis-paste or an attempt to type something else into the wizard.
    if code.is_empty() || code.chars().count() > 1024 || code.chars().any(char::is_control) {
        return Err(ApiError::bad_request("that does not look like an authorization code"));
    }
    let Some(status) = state.drive.wizards.status(&capture_id) else {
        return Err(ApiError::not_found("capture not found"));
    };
    if status.status != SetupTokenState::Pending {
        return Err(ApiError::conflict("this capture is no longer waiting for a code"));
    }
    if !state.terminals.write_terminal(&status.terminal_id, &format!("{code}\r")) {
        return Err(ApiError::conflict("the wizard's terminal is gone"));
    }
    Ok(gotcha::Json(SendResult { accepted: true }))
}

/// Where a capture stands (never the token itself).
#[api(id = "setup_token_status", group = "accounts", summary = "Poll a setup-token capture")]
pub async fn setup_token_status(
    State(state): State<AppState>,
    ApiPath(capture_id): ApiPath<String>,
) -> Result<gotcha::Json<SetupTokenStatus>, ApiError> {
    state
        .drive
        .wizards
        .status(&capture_id)
        .map(gotcha::Json)
        .ok_or_else(|| ApiError::not_found("capture not found"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESC: char = '\u{1b}';
    const BEL: char = '\u{7}';

    fn token() -> String {
        format!("sk-ant-oat01-{}", "A".repeat(70))
    }

    /// A long token, so a wrap can cut it somewhere plausible.
    fn long_token() -> String {
        format!("sk-ant-oat01-{}", "AbCd_1234-".repeat(9))
    }

    /// The reading the screen literally showed.
    fn printed(found: Option<TokenMatch>) -> Option<String> {
        found.map(|m| m.printed().to_string())
    }

    /// A capture whose two readings are the truncated line and the whole token.
    fn wrapped_capture() -> TokenMatch {
        let token = long_token();
        let mut scanner = TokenScanner::default();
        let found = scanner
            .push(&format!("{}{ESC}[2mStore this token somewhere safe{ESC}[0m\r\r\n", hard_wrap(&token, 80)))
            .expect("a token");
        assert_eq!(found.candidates().len(), 2);
        found
    }

    /// Play a fixed list of verdicts, one per check, and record what was asked.
    fn scripted(verdicts: Vec<Verdict>) -> (impl Fn(String) -> std::future::Ready<Verdict>, Arc<Mutex<Vec<String>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::new(Mutex::new(verdicts.into_iter()));
        let seen = asked.clone();
        let check = move |candidate: String| {
            seen.lock().expect("asked").push(candidate);
            let verdict = queue.lock().expect("queue").next().expect("one verdict per check");
            std::future::ready(verdict)
        };
        (check, asked)
    }

    fn decide(found: &TokenMatch, verdicts: Vec<Verdict>) -> (Result<Reading, String>, Vec<String>) {
        let (check, asked) = scripted(verdicts);
        let outcome = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(choose_reading(found, check));
        let asked = asked.lock().expect("asked").clone();
        (outcome, asked)
    }

    /// Break `text` into `cols`-wide lines the way the CLI's own wrapper does.
    fn hard_wrap(text: &str, cols: usize) -> String {
        text.as_bytes()
            .chunks(cols)
            .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
            .collect::<Vec<_>>()
            .join("\r\r\n")
    }

    #[test]
    fn strip_ansi_replaces_every_escape_with_a_space_instead_of_deleting_it() {
        assert_eq!(strip_ansi(&format!("red{ESC}[31mgreen")), "red green");
        assert_eq!(strip_ansi(&format!("{ESC}[2J{ESC}[Hplain")), "  plain");
        // OSC title sequences end in BEL and must not swallow the text after them.
        assert_eq!(strip_ansi(&format!("{ESC}]0;a title{BEL}after")), " after");
    }

    #[test]
    fn the_scanner_finds_a_token_split_across_two_chunks() {
        let token = token();
        let body = &token["sk-ant-oat01-".len()..];
        let mut scanner = TokenScanner::default();
        assert_eq!(printed(scanner.push(&format!("token: sk-ant-oat01-{}", &body[..30]))), None);
        // A trailing newline is not the end of the token: the next line could be
        // the rest of it, so the reading only settles once real text follows.
        assert_eq!(printed(scanner.push(&format!("{}\r\n", &body[30..]))), None);
        assert_eq!(printed(scanner.push("Store this token somewhere safe\r\n")), Some(token));
    }

    #[test]
    fn a_whole_token_is_only_trusted_once_something_follows_it() {
        let token = token();
        let mut scanner = TokenScanner::default();
        // Still streaming: the tail could be more token, so nothing is claimed yet.
        assert_eq!(printed(scanner.push(&format!("token: {token}"))), None);
        // Nothing more is coming, so the last look takes it.
        assert_eq!(printed(scanner.flush()), Some(token));
    }

    #[test]
    fn an_ansi_wrapped_hint_right_after_the_token_is_not_spliced_onto_it() {
        // The regression this module's escape handling exists for: deleting the
        // escapes joined "Store this token…" to the token and produced an
        // account whose every request 401'd.
        let token = token();
        let mut scanner = TokenScanner::default();
        let chunk = format!("{token}{ESC}[2m{ESC}[3mStore this token somewhere safe{ESC}[0m\r\n");
        assert_eq!(printed(scanner.push(&chunk)), Some(token));
    }

    /// The 401 this module's rejoining exists for.
    ///
    /// `claude setup-token` hard-wraps at the pty's width, and before
    /// [`WIZARD_SIZE`] that width was whatever browser pane last attached. A
    /// token printed into an 80-column terminal arrives as two lines, and taking
    /// the first one stores a credential exactly as plausible as a whole one,
    /// which 401s forever.
    #[test]
    fn a_token_the_terminal_wrapped_is_offered_whole_as_well_as_short() {
        let token = long_token();
        let mut scanner = TokenScanner::default();
        let chunk = format!("{}{ESC}[2mStore this token somewhere safe{ESC}[0m\r\r\n", hard_wrap(&token, 80));

        let found = scanner.push(&chunk).expect("a token");
        assert_eq!(found.candidates().len(), 2, "{:?}", found.candidates());
        assert_eq!(found.printed().len(), 80, "the screen says 80 characters, so that is the first reading");
        assert_eq!(
            found.candidates()[1],
            token,
            "and the rejoined token is there for the live check to promote"
        );
    }

    /// A phone-width pane wraps the same token three times over — and none of the
    /// three lines is long enough to be mistaken for a whole token, so the
    /// rejoined reading is the only one there is.
    #[test]
    fn a_token_wrapped_more_than_once_is_read_whole() {
        let token = long_token();
        let mut scanner = TokenScanner::default();
        let chunk = format!("{}{ESC}[2mStore this token somewhere safe{ESC}[0m\r\r\n", hard_wrap(&token, 40));

        let found = scanner.push(&chunk).expect("a token");
        assert_eq!(found.candidates(), [token], "{:?}", found.candidates());
    }

    /// A blank line is a paragraph break, not a wrap.
    #[test]
    fn a_paragraph_after_the_token_is_never_joined_onto_it() {
        let token = token();
        let mut scanner = TokenScanner::default();
        let chunk = format!("{token}\r\n\r\nStore this token somewhere safe\r\n");
        let found = scanner.push(&chunk).expect("a token");
        assert_eq!(found.candidates(), [token]);
    }

    #[test]
    fn a_short_body_is_not_mistaken_for_a_token() {
        let mut scanner = TokenScanner::default();
        assert_eq!(printed(scanner.push("sk-ant-oat01-tooshort\r\n")), None);
        assert_eq!(printed(scanner.push(&format!("sk-ant-oat01-{}\r\n", "B".repeat(59)))), None);
        assert_eq!(printed(scanner.flush()), None, "nor are two short ones glued together");
    }

    #[test]
    fn the_buffer_never_grows_past_the_scan_cap() {
        let mut scanner = TokenScanner::default();
        for _ in 0..8 {
            // Multi-byte, so a naive byte cut would land mid-character.
            assert_eq!(printed(scanner.push(&"é".repeat(16 * 1024))), None);
        }
        assert!(scanner.buffer.len() <= SCAN_CAP + 4, "buffer grew to {}", scanner.buffer.len());
    }

    /// The URL is printed as a terminal hyperlink whose *visible* text is
    /// wrapped to the pty's width; the escape carries the whole thing.
    #[test]
    fn the_authorization_url_is_taken_from_the_hyperlink_whole() {
        let url = "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a&scope=user%3Ainference&state=EDR4cIjOHOlb";
        let visible = hard_wrap(url, 80);
        let mut scanner = TokenScanner::default();
        scanner.push(&format!(
            "Browser didn't open? Use the url below\r\n{ESC}]8;id=xf3kl4;{url}{BEL}{visible}{ESC}]8;;{BEL}\r\n"
        ));
        assert_eq!(scanner.auth_url().as_deref(), Some(url));
    }

    /// No hyperlink: the URL is read off the screen, but only when a whole one is
    /// on the screen to read.
    #[test]
    fn a_url_with_no_hyperlink_is_read_off_one_line() {
        let url = "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a&scope=user%3Ainference&state=EDR4cIjOHOlb";
        let printed = format!("Use the url below\r\n{url}\r\n\r\nPaste code here if prompted >\r\n");
        assert_eq!(find_auth_url(&printed).as_deref(), Some(url));
    }

    /// The shape that made rejoining a URL indefensible: the CLI prints its
    /// prompt on the very next line, with no blank line between. "Paste" joins
    /// across that break exactly as a wrapped continuation would, and the result
    /// is a URL the user is told to go and authorize that leads nowhere.
    ///
    /// So a wrapped URL is reported as nothing, and the UI keeps waiting — the
    /// terminal is one disclosure away, and half a URL is worse than none.
    #[test]
    fn a_url_the_terminal_wrapped_is_never_guessed_at() {
        let url = "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a&scope=user%3Ainference&state=EDR4cIjOHOlb";
        let glued = format!("{}\r\nPaste code here if prompted > ", hard_wrap(url, 80));
        assert_eq!(find_auth_url(&glued), None, "a word from the next line must never join the URL");

        // The same refusal when the continuation really is the rest of the URL:
        // nothing on the screen says which of the two this is.
        let wrapped = format!("{}\r\n\r\nPaste code here if prompted > ", hard_wrap(url, 80));
        assert_eq!(find_auth_url(&wrapped), None);

        // …and the hyperlink, which carries the URL whole, is unaffected by any
        // of it: that is the path the real CLI takes.
        let linked = format!("{ESC}]8;id=1;{url}{BEL}{}{ESC}]8;;{BEL}\r\nPaste code here > ", hard_wrap(url, 80));
        assert_eq!(find_auth_url(&linked).as_deref(), Some(url));
    }

    #[test]
    fn half_a_hyperlink_is_not_a_url() {
        let partial = format!("{ESC}]8;id=1;https://claude.com/cai/oauth/authorize?code=true");
        assert_eq!(find_auth_url(&partial), None, "the sequence has not been terminated yet");
        assert_eq!(find_auth_url("nothing here"), None);
        assert_eq!(
            find_auth_url(&format!("{ESC}]8;;https://example.com/docs{BEL}docs{ESC}]8;;{BEL}")),
            None,
            "some other link the CLI printed is not the one to authorize"
        );
    }

    #[test]
    fn every_browser_opener_is_shadowed_by_a_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("bin");
        let vars = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(stub_browser_openers(&bin))
            .expect("stubs");

        let env: std::collections::BTreeMap<_, _> = vars.into_iter().collect();
        assert!(
            env["PATH"].starts_with(&format!("{}:", bin.display())),
            "the stubs have to win over the real openers: {}",
            env["PATH"]
        );
        assert_eq!(env["BROWSER"], bin.join("open").to_string_lossy());
        for name in BROWSER_OPENERS {
            let stub = bin.join(name);
            assert!(stub.is_file(), "{name} is not shadowed");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&stub).expect("stat").permissions().mode() & 0o111;
                assert_ne!(mode, 0, "{name} is not executable, so the real one would be found");
            }
        }
    }

    #[test]
    fn the_reading_anthropic_accepts_is_the_one_stored() {
        let found = wrapped_capture();
        let (outcome, asked) = decide(&found, vec![Verdict::Invalid("401".into()), Verdict::Valid]);
        assert_eq!(outcome.expect("stored").token, long_token());
        assert_eq!(asked.len(), 2, "the truncated reading is tried first and turned down");
    }

    /// The path that would otherwise write a known-bad token to disk: the
    /// truncated reading is *refused*, and the check on the whole one then hits a
    /// 429. A fallback to "what the screen said" would store the very reading
    /// Anthropic had just rejected.
    #[test]
    fn a_refused_reading_never_becomes_the_fallback_for_a_check_that_could_not_run() {
        let found = wrapped_capture();
        let (outcome, asked) = decide(&found, vec![Verdict::Invalid("401".into()), Verdict::Unverifiable("429".into())]);
        let error = outcome.expect_err("nothing may be stored");
        assert!(error.contains("429"), "the reason the user has to act on: {error}");
        assert!(error.contains("run the wizard again"), "{error}");
        assert_eq!(asked.len(), 2);
        assert_ne!(error, found.printed(), "and certainly not the truncated reading");
    }

    /// The re-authorization case: the account's quota ran out, so the very first
    /// check comes back 429. With two readings on the table there is nothing to
    /// choose between them, and a coin flip here is a silent 401 later.
    #[test]
    fn an_unrunnable_check_stores_nothing_when_the_readings_disagree() {
        let found = wrapped_capture();
        let (outcome, asked) = decide(&found, vec![Verdict::Unverifiable("rate-limited".into())]);
        assert!(outcome.is_err(), "{outcome:?}");
        assert_eq!(asked.len(), 1, "a second probe would break the same way");
    }

    /// …but one unambiguous reading on a machine that cannot check anything is
    /// the machine's problem, not the token's.
    #[test]
    fn an_unrunnable_check_still_stores_the_only_reading_there_is() {
        let mut scanner = TokenScanner::default();
        let found = scanner.push(&format!("{}{ESC}[2mStore this{ESC}[0m\r\n", token())).expect("a token");
        assert_eq!(found.candidates().len(), 1);
        let (outcome, _) = decide(&found, vec![Verdict::Unverifiable("no CLI".into())]);
        let stored = outcome.expect("stored");
        assert_eq!(stored.token, token());
        assert_eq!(stored.unverified.as_deref(), Some("no CLI"), "and the reply says nobody vouched for it");
    }

    #[test]
    fn every_reading_being_refused_reports_the_refusal() {
        let found = wrapped_capture();
        let (outcome, asked) = decide(
            &found,
            vec![Verdict::Invalid("first".into()), Verdict::Invalid("the token was refused: 401".into())],
        );
        assert_eq!(outcome.expect_err("refused"), "the token was refused: 401");
        assert_eq!(asked.len(), 2);
    }

    #[test]
    fn a_reused_capture_reports_the_flag_and_a_fresh_one_omits_it() {
        let captures = Captures::new();
        captures.insert("c1".to_string(), "t1".to_string(), PathBuf::from("/tmp/c1"));
        assert_eq!(captures.status("c1").expect("status").status, SetupTokenState::Pending);
        assert!(captures.status("c1").expect("status").reused.is_none());
        assert!(captures.status("c1").expect("status").auth_url.is_none());

        captures.saw_auth_url("c1", "https://claude.com/cai/oauth/authorize?x=1".to_string());
        assert_eq!(
            captures.status("c1").expect("status").auth_url,
            Optional(Some("https://claude.com/cai/oauth/authorize?x=1".to_string()))
        );

        captures.captured("c1", "acc".to_string(), true, None);
        let status = captures.status("c1").expect("status");
        assert_eq!(status.status, SetupTokenState::Captured);
        assert_eq!(status.reused, Optional(Some(true)));
        assert_eq!(status.account_id, Optional(Some("acc".to_string())));

        // A stored token stays `captured` however the CLI then went away.
        captures.note_exit("c1", 0);
        assert_eq!(captures.status("c1").expect("status").status, SetupTokenState::Captured);
        assert!(captures.status("c1").expect("status").exit_code.is_none());

        captures.remove("c1");
        assert!(captures.status("c1").is_none());
        assert!(captures.is_empty());
    }

    #[test]
    fn a_capture_that_never_saw_a_token_reports_the_exit_code() {
        let captures = Captures::new();
        captures.insert("c2".to_string(), "t2".to_string(), PathBuf::from("/tmp/c2"));
        captures.note_exit("c2", 3);
        let status = captures.status("c2").expect("status");
        assert_eq!(status.status, SetupTokenState::Exited);
        assert_eq!(status.exit_code, Optional(Some(3)));
    }
}
