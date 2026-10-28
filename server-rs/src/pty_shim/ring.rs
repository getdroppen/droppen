//! What a shim remembers: a bounded ring of raw output and a vt100 screen.
//!
//! Two things have to be true at once for a reattach to be honest.
//!
//! * **Nothing is silently invented.** The ring holds the last [`RING_CAP`]
//!   bytes and knows the absolute offset of the first byte it still has. A
//!   client resuming from outside that window — below it because the output
//!   fell out, above it because the cursor it kept cannot be true — is told, in
//!   a [`crate::pty_shim::proto::GAP`] frame, that the stream it is about to be
//!   handed is not the continuation of the one it had. See [`Ring::resume_at`].
//! * **The screen is reproducible even when the ring is not enough.** A `vim` an
//!   hour into a session cannot be reconstructed by replaying the tail of its
//!   output; it can be reconstructed from the *state* that output produced. So
//!   every byte is also fed to a `vt100::Parser`, and a snapshot is the escape
//!   sequences that repaint that screen.
//!
//! The two are updated under one lock, in one call ([`Recorder::append`]), which
//! is the invariant everything else leans on: any offset the ring reports is an
//! offset the parser has already seen. Split them and a snapshot taken between
//! the two writes describes a screen that never existed. The one thing that can
//! break the pairing is a parse that panics, and [`Recorder::append`] sets out
//! what is kept and what is thrown away when it does.

use std::collections::VecDeque;

/// How much raw output a shim keeps.
///
/// Enough that a server restarting under any normal circumstance resumes without
/// a gap, and small enough that a hundred idle terminals are not a hundred
/// hundred-megabyte processes.
pub const RING_CAP: usize = 8 * 1024 * 1024;

/// How many rows a fresh pty (and its parser) starts with.
pub const DEFAULT_ROWS: u16 = 24;
/// How many columns a fresh pty (and its parser) starts with.
pub const DEFAULT_COLS: u16 = 80;

/// A bounded byte ring addressed by absolute offset.
#[derive(Debug)]
pub struct Ring {
    bytes: VecDeque<u8>,
    /// Absolute offset of `bytes[0]`.
    base: u64,
    cap: usize,
}

impl Ring {
    /// An empty ring holding at most `cap` bytes.
    pub fn new(cap: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            base: 0,
            cap,
        }
    }

    /// The absolute offset of the oldest byte still held.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// The absolute offset one past the newest byte — i.e. how much output the
    /// shell has produced in its whole life.
    pub fn total(&self) -> u64 {
        self.base + self.bytes.len() as u64
    }

    /// Append output, dropping from the front once past the cap.
    pub fn append(&mut self, chunk: &[u8]) {
        // A single write larger than the whole ring: keep its tail, and account
        // for every byte skipped so `base` still tells the truth.
        let chunk = if chunk.len() > self.cap {
            let skipped = chunk.len() - self.cap;
            self.base += skipped as u64;
            &chunk[skipped..]
        } else {
            chunk
        };
        self.bytes.extend(chunk.iter().copied());
        let over = self.bytes.len().saturating_sub(self.cap);
        if over > 0 {
            self.bytes.drain(..over);
            self.base += over as u64;
        }
    }

    /// Everything from `cursor` on, clamped into what is still held.
    ///
    /// A cursor below [`Ring::base`] yields the oldest bytes there are; the
    /// caller is responsible for having announced the gap first.
    pub fn read_from(&self, cursor: u64) -> Vec<u8> {
        let start = cursor.max(self.base).min(self.total());
        let offset = (start - self.base) as usize;
        let (front, back) = self.bytes.as_slices();
        let mut out = Vec::with_capacity(self.bytes.len() - offset);
        if offset < front.len() {
            out.extend_from_slice(&front[offset..]);
            out.extend_from_slice(back);
        } else {
            out.extend_from_slice(&back[offset - front.len()..]);
        }
        out
    }

    /// Where a client resuming at `cursor` actually lands, and what it has to be
    /// told before it gets there.
    ///
    /// Two cursors are impossible rather than merely stale, and both are
    /// answered the same way, because the client's next byte is discontinuous
    /// either way and saying so is the only thing that keeps a scrollback
    /// honest:
    ///
    /// * below [`Ring::base`], the output it is asking for fell out of the ring
    ///   — the everyday case, and it resumes at the oldest byte still held;
    /// * above [`Ring::total`], it claims to have read output the shell never
    ///   produced. Nothing but a truncated or garbled `.cursor` file says that,
    ///   and trusting it means skipping every byte between the ring's head and
    ///   whatever number was in the file. It resumes at the head instead.
    ///
    /// The gap announced for the second case runs backwards — `from` above `to`
    /// — which [`super::proto::Gap::lost`] already reports as zero bytes. Zero
    /// is the honest number: nothing was lost, a record was wrong, and the note
    /// is there to mark the discontinuity rather than to measure it.
    pub fn resume_at(&self, cursor: u64) -> (u64, Option<super::proto::Gap>) {
        let landing = cursor.clamp(self.base, self.total());
        let gap = (landing != cursor).then_some(super::proto::Gap { from: cursor, to: landing });
        (landing, gap)
    }
}

/// What the parser made of what it was handed.
///
/// Returned rather than swallowed because the caller is the only one that can
/// say it out loud: a screen model that has been rebuilt is a terminal whose
/// repaint is now poorer than its output, and that is worth a line in a log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Parsed {
    /// The parser took it. The ring and the screen agree, as they must.
    Screen,
    /// The parser did not come back and was rebuilt. See [`Recorder::append`].
    Reset,
}

/// The ring plus the screen its bytes produced.
pub struct Recorder {
    ring: Ring,
    parser: vt100::Parser,
    /// The window the parser is modelling, kept here as well.
    ///
    /// Not read back off the parser, because the one moment it is needed is
    /// the moment the parser has just panicked and is not to be trusted for
    /// anything, its own dimensions included.
    size: (u16, u16),
    /// How many times the parser has had to be rebuilt. Nothing but a
    /// post-mortem reads it, which is the point: it should always be zero.
    faults: u64,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder")
            .field("base", &self.ring.base())
            .field("total", &self.ring.total())
            // Always zero in a healthy shim, and the first thing worth knowing
            // about one that is not.
            .field("parser_faults", &self.faults)
            .finish_non_exhaustive()
    }
}

impl Recorder {
    /// A recorder for a window of `rows` by `cols`.
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            ring: Ring::new(RING_CAP),
            // No scrollback: a snapshot repaints the visible screen, and the
            // scrolled-off history is the ring's job, not the parser's.
            parser: vt100::Parser::new(rows, cols, 0),
            size: (rows, cols),
            faults: 0,
        }
    }

    /// The ring, for offsets and reads.
    pub fn ring(&self) -> &Ring {
        &self.ring
    }

    /// How many times the parser has been rebuilt under [`Recorder::append`].
    pub fn parser_faults(&self) -> u64 {
        self.faults
    }

    /// Record output: into the ring and through the parser, in that order and
    /// under the caller's single lock.
    ///
    /// # What a parse that does not come back leaves behind
    ///
    /// `vt100::Parser::process` is third-party code handed, by definition,
    /// whatever the shell decided to print — which is whatever the program the
    /// user ran decided to print, i.e. arbitrary bytes. A terminal emulator has
    /// to survive that, and when it does not, the unwind would go straight
    /// through the reader thread *while it held this lock*. That would kill the
    /// shim twice over: nothing would be left reading the pty, and the poisoned
    /// mutex would make every later client's `HELLO` panic before its reply
    /// went out. So the parse is caught here, where the recorder's own
    /// invariants are the ones being put back.
    ///
    /// What the recorder holds afterwards is a deliberate choice of three:
    ///
    /// * **The ring keeps the chunk.** Those bytes really were written by the
    ///   shell, `total` has already counted them, and a ring cannot un-append.
    ///   Dropping them would make every later offset name a byte other than the
    ///   one it names now — a silent discontinuity, which is the one thing this
    ///   module refuses to produce.
    /// * **The parser is rebuilt empty, at the same size.** After an unwind out
    ///   of the middle of an escape sequence its state is whatever it happened
    ///   to be, and a snapshot taken from it would describe a screen that never
    ///   existed — exactly what [`Recorder::append`] holds one lock to prevent.
    ///   An empty screen claims nothing instead of claiming something false.
    /// * **It is not replayed.** The bytes that killed the parser are in the
    ///   ring, so replaying the ring would hand them straight back to it, and
    ///   the thread that does the replaying is the one thread reading the pty.
    ///   The screen refills from the next chunk the shell writes.
    ///
    /// The cost is bounded to the repaint: a client's live stream is raw ring
    /// bytes, which its own emulator renders, and a client attaching from
    /// cursor zero is sent the whole ring before any snapshot. What is lost is
    /// the shim's *opinion* of the screen until the shell next redraws it.
    pub fn append(&mut self, chunk: &[u8]) -> Parsed {
        self.ring.append(chunk);
        self.parse(|parser| parser.process(chunk))
    }

    /// Tell the parser the window changed. The pty itself is resized by the
    /// caller; this keeps the snapshot the right shape.
    pub fn resize(&mut self, rows: u16, cols: u16) -> Parsed {
        self.size = (rows, cols);
        self.parse(|parser| parser.screen_mut().set_size(rows, cols))
    }

    /// Hand the parser some work, and rebuild it if it does not come back.
    fn parse(&mut self, work: impl FnOnce(&mut vt100::Parser)) -> Parsed {
        // `AssertUnwindSafe` is the honest claim here rather than a shrug: the
        // parser is the only thing the closure can leave inconsistent, and the
        // recovery is to throw that parser away.
        let parser = &mut self.parser;
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || work(parser))).is_ok() {
            return Parsed::Screen;
        }
        let (rows, cols) = self.size;
        self.parser = vt100::Parser::new(rows, cols, 0);
        self.faults += 1;
        Parsed::Reset
    }

    /// [`Recorder::append`] with a parse that panics.
    ///
    /// The seam is here because the failure cannot be produced on demand: it is
    /// whatever byte sequence `vt100` mishandles, and if that were known it
    /// would be fixed rather than guarded. What is testable is everything the
    /// guard promises about the recorder afterwards.
    #[cfg(test)]
    fn append_through_a_failing_parse(&mut self, chunk: &[u8]) -> Parsed {
        self.ring.append(chunk);
        self.parse(|_| panic!("deliberate: a parser that does not come back"))
    }

    /// Escape sequences that repaint the current screen from scratch.
    ///
    /// `state_formatted` clears attributes and the screen, repaints every cell,
    /// places the cursor and then sets the input modes — application cursor
    /// keys, application keypad, bracketed paste and the mouse protocol — with
    /// an explicit *off* sequence for each mode that is off, so a client whose
    /// terminal is in the wrong mode is corrected rather than left there.
    ///
    /// The alternate screen is the one piece of state it does not carry, because
    /// `vt100` models the two screens rather than the switch between them. So it
    /// is written here, and written in both directions for the same reason the
    /// modes are: reattaching to a shell that has *left* `vim` must take the
    /// client's terminal out of the alternate screen, not leave it there.
    pub fn snapshot(&self) -> Vec<u8> {
        let screen = self.parser.screen();
        let mut out = Vec::new();
        out.extend_from_slice(if screen.alternate_screen() {
            b"\x1b[?1049h".as_slice()
        } else {
            b"\x1b[?1049l".as_slice()
        });
        out.extend_from_slice(&screen.state_formatted());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`Ring::resume_at`] as something an assertion can compare: where the
    /// client lands, and the gap flattened to its two ends.
    fn resume(ring: &Ring, cursor: u64) -> (u64, Option<(u64, u64)>) {
        let (landing, gap) = ring.resume_at(cursor);
        (landing, gap.map(|gap| (gap.from, gap.to)))
    }

    #[test]
    fn the_ring_tracks_absolute_offsets_across_an_overflow() {
        let mut ring = Ring::new(8);
        ring.append(b"abcd");
        assert_eq!((ring.base(), ring.total()), (0, 4));
        assert_eq!(ring.read_from(0), b"abcd");
        assert_eq!(ring.read_from(2), b"cd");
        assert_eq!(resume(&ring, 0), (0, None));

        ring.append(b"efghij");
        // Ten bytes written, eight held: "cdefghij".
        assert_eq!((ring.base(), ring.total()), (2, 10));
        assert_eq!(ring.read_from(2), b"cdefghij");
        assert_eq!(ring.read_from(9), b"j");
        assert_eq!(ring.read_from(10), b"", "a cursor at the head reads nothing, not a panic");
        assert_eq!(ring.read_from(99), b"", "and neither does one past it");

        assert_eq!(resume(&ring, 0), (2, Some((0, 2))), "below base is a gap, and it lands on base");
        assert_eq!(ring.resume_at(0).1.expect("gap").lost(), 2);
        assert_eq!(resume(&ring, 2), (2, None), "a cursor exactly at base is not a gap");
        assert_eq!(resume(&ring, 10), (10, None), "and neither is one exactly at the head");
        // Falling into a gap still yields the oldest bytes held, so a client can
        // resume after being told what it lost.
        assert_eq!(ring.read_from(0), b"cdefghij");
    }

    /// A `.cursor` file that was truncated or garbled can name an offset the
    /// shell never reached. Trusting it means skipping every byte from here to
    /// there, silently — so it is clamped to the head instead, and announced the
    /// way a real gap is, because the stream is discontinuous either way.
    #[test]
    fn a_cursor_beyond_anything_the_shell_wrote_is_clamped_rather_than_believed() {
        let mut ring = Ring::new(8);
        ring.append(b"abcd");

        assert_eq!(
            resume(&ring, 9_999_999),
            (4, Some((9_999_999, 4))),
            "it resumes at the head, not at a number out of a broken file, and says so"
        );
        let gap = ring.resume_at(9_999_999).1.expect("a discontinuity has to be said out loud");
        assert_eq!(gap.lost(), 0, "nothing was lost; a record was wrong");

        // And from there it behaves like any other client: the next byte the
        // shell writes is the next byte it is handed.
        ring.append(b"ef");
        assert_eq!(ring.read_from(4), b"ef");
        assert_eq!(resume(&ring, u64::MAX).0, 6, "the peek idiom lands on the head too");
    }

    #[test]
    fn one_write_larger_than_the_ring_keeps_its_tail_and_still_counts_every_byte() {
        let mut ring = Ring::new(4);
        ring.append(b"0123456789");
        assert_eq!((ring.base(), ring.total()), (6, 10));
        assert_eq!(ring.read_from(6), b"6789");
        assert_eq!(ring.resume_at(0).1.expect("gap").lost(), 6);
    }

    #[test]
    fn the_snapshot_repaints_the_screen_and_states_every_mode() {
        let mut rec = Recorder::new(4, 20);
        assert_eq!(rec.append(b"hello"), Parsed::Screen);
        let snap = String::from_utf8(rec.snapshot()).expect("utf8");

        assert!(snap.contains("hello"), "the repaint carries the screen contents: {snap:?}");
        assert!(snap.contains("\x1b[H\x1b[J"), "the repaint clears before it paints: {snap:?}");
        // Every mode is written even when it is off, so a client that is in the
        // wrong one is corrected rather than left there.
        assert!(snap.contains("\x1b[?1049l"), "not on the alternate screen: {snap:?}");
        assert!(snap.contains("\x1b[?1l"), "application cursor off: {snap:?}");
        assert!(snap.contains("\x1b>"), "application keypad off: {snap:?}");
        assert!(snap.contains("\x1b[?2004l"), "bracketed paste off: {snap:?}");
    }

    #[test]
    fn the_snapshot_carries_the_modes_a_full_screen_program_turned_on() {
        let mut rec = Recorder::new(4, 20);
        // What a `vim` does on entry: alternate screen, application cursor keys,
        // application keypad, bracketed paste.
        assert_eq!(rec.append(b"\x1b[?1049h\x1b[?1h\x1b=\x1b[?2004h~ INSERT --"), Parsed::Screen);
        let snap = String::from_utf8(rec.snapshot()).expect("utf8");

        assert!(snap.starts_with("\x1b[?1049h"), "the alternate screen is entered first: {snap:?}");
        assert!(snap.contains("INSERT"), "the alternate screen's contents are repainted: {snap:?}");
        assert!(snap.contains("\x1b[?1h"), "application cursor on: {snap:?}");
        assert!(snap.contains("\x1b="), "application keypad on: {snap:?}");
        assert!(snap.contains("\x1b[?2004h"), "bracketed paste on: {snap:?}");

        // And leaving turns every one of them back off.
        assert_eq!(rec.append(b"\x1b[?2004l\x1b>\x1b[?1l\x1b[?1049l"), Parsed::Screen);
        let snap = String::from_utf8(rec.snapshot()).expect("utf8");
        assert!(snap.starts_with("\x1b[?1049l"), "the alternate screen is left: {snap:?}");
        assert!(snap.contains("\x1b[?2004l") && snap.contains("\x1b[?1l") && snap.contains("\x1b>"));
        assert!(!snap.contains("INSERT"), "and the alternate screen's contents go with it: {snap:?}");
    }

    /// A parse that panics must cost the screen and nothing else: the bytes stay
    /// in the ring at the offsets clients know, and the recorder comes out of it
    /// usable and honest about what it kept.
    #[test]
    fn a_parse_that_panics_costs_the_screen_and_nothing_else() {
        let mut rec = Recorder::new(4, 20);
        assert_eq!(rec.append(b"before"), Parsed::Screen);

        assert_eq!(rec.append_through_a_failing_parse(b"hostile"), Parsed::Reset);
        assert_eq!(rec.parser_faults(), 1);

        // The bytes are still the shell's own: counted, readable, and at the
        // offsets every client already knows them by.
        assert_eq!(rec.ring().total(), 13);
        assert_eq!(rec.ring().read_from(0), b"beforehostile");
        assert_eq!(resume(rec.ring(), 13), (13, None), "no client is told the stream jumped");

        // The screen is empty rather than wrong, and it is still a screen: the
        // snapshot works, at the same size, and the next output lands on it.
        let snap = String::from_utf8(rec.snapshot()).expect("utf8");
        assert!(!snap.contains("before"), "a rebuilt parser claims nothing: {snap:?}");
        assert_eq!(rec.append(b"after"), Parsed::Screen, "and it goes on recording");
        let snap = String::from_utf8(rec.snapshot()).expect("utf8");
        assert!(snap.contains("after"), "the screen refills from what the shell writes next: {snap:?}");

        // A resize is the other call that reaches into the parser, and it
        // survives the same way — including keeping the size the caller asked
        // for, which is what the rebuilt parser is given.
        assert_eq!(rec.resize(6, 10), Parsed::Screen);
        assert_eq!(rec.append_through_a_failing_parse(b"again"), Parsed::Reset);
        assert_eq!(rec.parser_faults(), 2);
        assert_eq!(
            rec.parser.screen().size(),
            (6, 10),
            "the rebuilt parser is the window the caller last asked for, not the one it started at"
        );
        assert_eq!(rec.append(b"after the second"), Parsed::Screen);
        assert!(String::from_utf8_lossy(&rec.snapshot()).contains("after the second"));
    }

    #[test]
    fn a_resize_reshapes_the_snapshot_grid() {
        let mut rec = Recorder::new(2, 4);
        assert_eq!(rec.append(b"abcdefgh"), Parsed::Screen);
        assert_eq!(rec.resize(4, 8), Parsed::Screen);
        assert_eq!(rec.append(b"XY"), Parsed::Screen);
        let snap = String::from_utf8(rec.snapshot()).expect("utf8");
        assert!(snap.contains("XY"), "output after the resize lands on the new grid: {snap:?}");
        // The ring is untouched by a resize: it is bytes, not a screen.
        assert_eq!(rec.ring().total(), 10);
    }
}
