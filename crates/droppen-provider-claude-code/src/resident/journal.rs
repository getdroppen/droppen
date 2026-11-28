//! Run journal row payload (`ClaudeRun`), queued entries, and `ClaudeJournal` helpers.

use super::{Continuation, Fingerprint, Hold};
use droppen_provider::runs;

/// What a Claude Code run keeps in its journal row, beside what every run
/// says ([`runs::RunRecord`]): written after those fields, in this order.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct ClaudeRun {
    /// What this resident's command line settled: model, effort, permission
    /// mode, account, persona, uploads.
    ///
    /// Recorded because a re-attached resident has to answer "may this message
    /// go to the process that is already running" — and the answer is a
    /// comparison against the settings the process was *started* with, which
    /// nothing else on disk remembers. Getting it wrong in either direction
    /// costs: a false mismatch throws a warm session away on the first message
    /// after every restart, and a false match sends an opus message to a
    /// haiku process.
    ///
    /// Every field is a setting the user chose, and none is a secret. `None`
    /// in a record from before this existed, which reads as "unknown" and is
    /// handled as such rather than as "no settings at all" — see
    /// [`Fingerprint`].
    ///
    /// Written at the spawn and then *healed* rather than left alone: an
    /// unknown one is filled in by the first message an adopted resident is
    /// given, and the model is rewritten when `system:init` resolves the alias
    /// the command line asked for into the name the CLI answers under. Both are
    /// [`ClaudeJournal::mark_fingerprint`], and both exist so that the record the
    /// next server reads is what the process is really running rather than what
    /// some older server managed to write down.
    #[serde(default)]
    pub(crate) fingerprint: Option<Fingerprint>,
    /// Where in claude's stdout a turn was written into this resident, if one
    /// was still in flight when this was last written.
    ///
    /// The one thing a re-attached server cannot work out for itself. A turn
    /// begins when a prompt is written to claude's *stdin*, and the shim records
    /// only its stdout — so a turn that has said nothing yet, which is every
    /// turn spent thinking or inside a long tool call, is invisible in the
    /// replay. Without this a restart in that window reads the session as idle,
    /// unblocks the composer, and lets the next message be written into the
    /// middle of a turn that is very much still running.
    ///
    /// A byte offset rather than a flag, because *when* is the whole question: a
    /// turn that ended while no server was attached left its `result` in the
    /// shim's ring, and adoption has to be able to tell that `result` from the
    /// ones belonging to earlier turns. The offset is the reader's cursor at the
    /// moment the prompt went in, so everything before it is history and the
    /// first `result` after it is this turn's end. `None` means "no turn was in
    /// flight", which is also what every record written before this existed says.
    #[serde(default)]
    pub(crate) turn_at: Option<u64>,
    /// The messages accepted for this resident and not yet written to it.
    ///
    /// The other half of what used to die with a server. A send is answered
    /// `200` the moment it is queued — that is what lets a second message be
    /// composed while the first is still running — and the queue itself was in
    /// memory, so a restart between "accepted" and "written to claude's stdin"
    /// threw the message away in silence. That window is not small: a message
    /// sent behind a turn sits in it for as long as the turn takes.
    ///
    /// In order, popped as each one goes in, and empty the rest of the time.
    #[serde(default)]
    pub(crate) queued: Vec<Queued>,
    /// A claude resident's queue held after a limit with no account to move
    /// to, so the next server keeps holding it rather than feeding the queue
    /// to the account that refused. See [`Hold`]. Absent on
    /// every record that is not holding anything, and on every record written
    /// before there was a hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) hold: Option<Hold>,
}

/// One message waiting its turn, as it survives on disk.
///
/// The prompt is here in plain text, which is a line worth saying out loud why
/// it is crossed: it is one step away from being written into the session
/// transcript — a plain file in the same data directory, holding every prompt
/// the session has ever had — so this reveals nothing the transcript does not,
/// and it holds it only until claude has it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Queued {
    /// What the queue routes name this message by, kept across a restart so a
    /// client holding an id from before it still holds a usable one.
    ///
    /// Empty in every record written before the queue was addressable, which
    /// a re-attached resident answers by minting a fresh id on the way back in:
    /// the message survives, only its name changes.
    #[serde(default)]
    pub(crate) id: String,
    /// The prompt as claude will see it, attachment suffix included.
    pub(crate) text: String,
    /// Upload paths whose bytes become image blocks at delivery, re-fenced and
    /// re-read then. Absent on every record written before chats carried
    /// images, and on every session's message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) images: Vec<String>,
    /// Upload paths whose UTF-8 contents are inlined into the prompt at
    /// delivery. Absent for sessions and for records written before chats
    /// carried files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) files: Vec<String>,
    /// The process this message needs, so a re-attached server can tell whether
    /// the resident it inherited may answer it at all.
    ///
    /// The environment is not here and cannot be — it carries the account's
    /// token — so a message that does *not* fit the running process cannot be
    /// re-delivered by a server that did not start that process. Recording what
    /// it wanted is what lets adoption notice, and say so, instead of sending an
    /// opus message to a haiku process.
    #[serde(default)]
    pub(crate) fingerprint: Fingerprint,
    /// Whether this message had already been written to the running process'
    /// stdin, ahead of its turn, when this was recorded.
    ///
    /// The difference matters to exactly one reader: the server that re-attaches
    /// to the process this record names. That process still holds the line in
    /// its stdin buffer and will run it, so re-delivering it would run it twice
    /// — see [`super::inherited_queue`]. A record read back against
    /// any *other* process (there is no such path today) would have to treat it
    /// as undelivered.
    #[serde(default)]
    pub(crate) steered: bool,
    /// How many times an automatic account rotation has already moved the
    /// message chain this one belongs to: `0` for anything a person sent, and
    /// the count for the continuation a rotation queued in its place. Kept so
    /// the cap in [`droppen_accounts::rotation::ROTATION_CAP`] survives a
    /// restart; absent, and zero, on every record written before there was one.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) rotation_attempt: u8,
    /// For a rotation's continuation, what it needs to say which background
    /// tasks its respawn cut short — kept so a continuation parked behind
    /// background work survives a restart whole. Absent on everything else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) continuation: Option<Continuation>,
}

/// Whether a counter is at its default, so a record that never rotated is
/// written exactly as it always was.
fn is_zero(value: &u8) -> bool {
    *value == 0
}

/// Whether a flag is off, so a row that never had it is written exactly as it
/// always was — while a `true` survives every patch of the row.
pub(super) fn is_false(value: &bool) -> bool {
    !*value
}

/// What a Claude Code resident's driver writes into its run's row as it goes:
/// each a change to the one row, in one transaction ([`runs::RunsFile`]).
pub(crate) trait ClaudeJournal {
    /// Say what a resident is in the middle of: the turn in flight, and what is
    /// still waiting behind it.
    ///
    /// Both in one transaction, and that is the point rather than an
    /// optimisation. Handing a message to claude moves it out of the queue *and*
    /// opens a turn, and a crash between two separate commits would leave the
    /// record claiming either that a delivered message is still waiting — so the
    /// next boot sends it twice — or that neither is happening, which unblocks
    /// the composer on a live turn. One transaction has neither state.
    fn mark_progress(&self, run_id: &str, turn_at: Option<u64>, queued: &[Queued]);

    /// Say whether this resident's queue is held after a limit, and on what.
    fn mark_hold(&self, run_id: &str, hold: Option<Hold>);

    /// Say what the process this record names is really running.
    ///
    /// A write of its own rather than another argument to
    /// [`ClaudeJournal::mark_progress`], because it happens on a different rhythm:
    /// the settings are fixed for a process' whole life and are recorded at the
    /// spawn, so this is only ever a *correction* — the record that predates the
    /// field and whose driver has since been told what it is holding, and the
    /// model alias `system:init` resolves. Rare, and each one saves the next
    /// server from having to learn the same thing again.
    ///
    /// The account is part of what is written, and is the reason this is not
    /// simply left to the next spawn: a record that says nothing is one the next
    /// boot cannot compare a message against, and comparing against the default
    /// instead is what threw queued messages away after an upgrade.
    fn mark_fingerprint(&self, run_id: &str, fingerprint: &Fingerprint);

    /// Point a run at the session id the CLI actually chose.
    ///
    /// Claude's `--fork-session` adopt path needs this: the CLI may keep the
    /// session under an id of its own choosing rather than the one the fork
    /// asked for, and announces it on `system:init`, so a run is recorded under
    /// a provisional id first. Without the rewrite, adoption would re-arm the
    /// idle watchdog on a transcript that does not exist.
    fn repoint(&self, run_id: &str, session_id: &str);
}

impl ClaudeJournal for runs::RunsFile {
    fn mark_progress(&self, run_id: &str, turn_at: Option<u64>, queued: &[Queued]) {
        let queued = queued.to_vec();
        self.patch("progress on a run", run_id, move |run: &mut runs::RunRecord<ClaudeRun>| {
            run.payload.turn_at = turn_at;
            run.payload.queued = queued;
        });
    }

    fn mark_hold(&self, run_id: &str, hold: Option<Hold>) {
        self.patch("a held queue", run_id, move |run: &mut runs::RunRecord<ClaudeRun>| {
            run.payload.hold = hold;
        });
    }

    fn mark_fingerprint(&self, run_id: &str, fingerprint: &Fingerprint) {
        let fingerprint = fingerprint.clone();
        self.patch("what a run is holding", run_id, move |run: &mut runs::RunRecord<ClaudeRun>| {
            run.payload.fingerprint = Some(fingerprint);
        });
    }

    fn repoint(&self, run_id: &str, session_id: &str) {
        let session_id = session_id.to_string();
        self.patch("the session a run belongs to", run_id, move |run: &mut runs::RunRecord<ClaudeRun>| {
            run.session_id = session_id;
        });
    }
}

/// A Claude Code row put in a journal as Claude Code reads and writes it, for
/// the server's tests of each CLI's rows side by side. The row's type stays
/// this crate's own: outside it, a test hands the row over as text.
#[cfg(any(test, feature = "test-support"))]
pub trait ClaudeRowsExt {
    /// Add `row`, read as a Claude Code run. Panics if it is not one.
    fn add_claude_row(&self, row: &str);
}

#[cfg(any(test, feature = "test-support"))]
impl ClaudeRowsExt for runs::RunsFile {
    fn add_claude_row(&self, row: &str) {
        self.add(serde_json::from_str::<runs::RunRecord<ClaudeRun>>(row).expect("a Claude Code row"));
    }
}
