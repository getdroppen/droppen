//! A transcript's items as a parse makes them, and how far back it changed
//! them.
//!
//! A parse appends items as it reads entries, and now and then changes one it
//! made earlier: a question its answer settles, a command its output arrives
//! for, a subagent's launch once its sidecar is on disk. A parse that goes on
//! from an earlier one ([`crate::resume`]) starts from that parse's items and
//! has to say which of them it changed, so that the fingerprint chain is made
//! again from there and not from the start ([`crate::loader`]). [`ItemLog`]
//! keeps that count itself: every way to change, take out or put in an item
//! counts.
//!
//! Taking out or putting in an item also moves the ones after it, which the
//! mentioned-file chips have walked already ([`crate::mentions::Mentions`]):
//! the log keeps the lowest index moved ([`ItemLog::moved_from`]), and
//! whether an item the annotation could chip was among those taken out or
//! moved, which only a walk from the first item can account for.
//!
//! What it cannot check is the parser's side of it. A parse that went on is
//! only right when the parser's own state (what it remembers of the entries
//! before) leads it to make exactly the changes a parse from the start makes;
//! that is what the parser's own tests prove against such a parse.
//!
//! Each item is behind an `Arc` of its own, as a [`crate::loader::Transcript`]
//! holds them, so going on from an earlier parse copies one pointer per item,
//! and changing an item copies that item alone.

use std::sync::Arc;

use super::util::TranscriptItem;

/// A transcript's items, being made, and the lowest index changed since the
/// items it started from.
#[derive(Debug, Default)]
pub struct ItemLog {
    items: Vec<Arc<TranscriptItem>>,
    /// Every item below it is the very one the log started with.
    changed_from: usize,
    /// The lowest index an item was taken out at or put in at; how many it
    /// started with when none was.
    moved_from: usize,
    /// Whether an item the annotation could chip was taken out or moved
    /// along. Counted whether or not it was walked yet: at worst, the walk
    /// starts over when it need not have.
    moved_annotatable: bool,
}

impl ItemLog {
    /// No items: a parse from the start, where everything is new.
    pub fn new() -> Self {
        Self::default()
    }

    /// The items of an earlier parse, to go on from: nothing is changed yet.
    pub fn resume(items: Vec<Arc<TranscriptItem>>) -> Self {
        Self {
            changed_from: items.len(),
            moved_from: items.len(),
            moved_annotatable: false,
            items,
        }
    }

    /// How many items there are.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The items, as a [`crate::loader::Transcript`] holds them.
    pub fn items(&self) -> &[Arc<TranscriptItem>] {
        &self.items
    }

    /// Item `index`.
    pub fn get(&self, index: usize) -> Option<&TranscriptItem> {
        self.items.get(index).map(|item| &**item)
    }

    /// The newest item.
    pub fn last(&self) -> Option<&TranscriptItem> {
        self.items.last().map(|item| &**item)
    }

    /// The lowest index whose item may differ from the one the log started
    /// with: its length for a log nothing changed, 0 for one from the start.
    pub fn changed_from(&self) -> usize {
        self.changed_from
    }

    /// Add `item` after the others.
    pub fn push(&mut self, item: TranscriptItem) {
        self.items.push(Arc::new(item));
    }

    /// Item `index`, to change: from here on, the items are not the ones the
    /// log started with.
    ///
    /// Never an item the annotation could chip (an assistant's text): what it
    /// keeps of the item as written ([`TranscriptItem::as_transcribed`])
    /// would then be the item from before the change, its fingerprint wrong,
    /// and the walk that chipped it not walked again. The check catches only
    /// an item that already has chips; the rest is the parser's to keep to.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut TranscriptItem> {
        let item = self.items.get_mut(index)?;
        debug_assert!(!item.is_annotated(), "item {index} was changed after it was annotated");
        self.changed_from = self.changed_from.min(index);
        Some(Arc::make_mut(item))
    }

    /// The newest item, to change; see [`ItemLog::get_mut`].
    pub fn last_mut(&mut self) -> Option<&mut TranscriptItem> {
        let last = self.items.len().checked_sub(1)?;
        self.get_mut(last)
    }

    /// Put `item` in place of item `index`; see [`ItemLog::get_mut`].
    pub fn replace(&mut self, index: usize, item: TranscriptItem) {
        if let Some(old) = self.get_mut(index) {
            *old = item;
        }
    }

    /// Take the newest item out.
    pub fn pop(&mut self) -> Option<TranscriptItem> {
        let item = self.items.pop()?;
        let index = self.items.len();
        debug_assert!(!item.is_annotated(), "item {index} was taken out after it was annotated");
        self.changed_from = self.changed_from.min(index);
        self.moved_from = self.moved_from.min(index);
        self.moved_annotatable |= item.is_annotatable();
        Some(Arc::unwrap_or_clone(item))
    }

    /// Put `items` in at `index`, moving what is there and after it along.
    /// Panics when `index` is past the end.
    pub fn insert(&mut self, index: usize, items: impl IntoIterator<Item = TranscriptItem>) {
        self.changed_from = self.changed_from.min(index);
        self.moved_from = self.moved_from.min(index);
        self.moved_annotatable |= self.items[index..].iter().any(|item| item.is_annotatable());
        self.items.splice(index..index, items.into_iter().map(Arc::new));
    }

    /// The lowest index an item was taken out at or put in at since the log
    /// started: the items from there on are not where they were. Its length
    /// then, when none was.
    pub fn moved_from(&self) -> usize {
        self.moved_from
    }

    /// Whether an item the annotation could chip was taken out or moved
    /// along ([`ItemLog::moved_from`]).
    pub(crate) fn moved_annotatable(&self) -> bool {
        self.moved_annotatable
    }

    /// Item `index`, for the mentioned-file chips alone ([`crate::mentions`]):
    /// a change the fingerprint chain leaves out, so the count does not move.
    pub(crate) fn annotate_mut(&mut self, index: usize) -> &mut TranscriptItem {
        Arc::make_mut(&mut self.items[index])
    }

    /// The items, and the lowest index changed ([`ItemLog::changed_from`]).
    pub fn into_parts(self) -> (Vec<Arc<TranscriptItem>>, usize) {
        (self.items, self.changed_from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{make_item, ItemKind, Role};

    fn item(text: &str) -> TranscriptItem {
        make_item(Role::Assistant, ItemKind::Text, text, None)
    }

    fn texts(log: &ItemLog) -> Vec<&str> {
        log.items().iter().map(|item| item.text.as_str()).collect()
    }

    /// From the start, everything is new; going on from earlier items,
    /// nothing is until something below the end is touched.
    #[test]
    fn a_log_counts_from_where_it_started() {
        let mut fresh = ItemLog::new();
        fresh.push(item("a"));
        assert_eq!(fresh.changed_from(), 0);

        let earlier = vec![Arc::new(item("a")), Arc::new(item("b"))];
        let mut log = ItemLog::resume(earlier.clone());
        assert_eq!(log.changed_from(), 2, "nothing changed");
        log.push(item("c"));
        assert_eq!(log.changed_from(), 2, "an append changes nothing before it");
        assert!(Arc::ptr_eq(&log.items()[0], &earlier[0]), "the earlier items are the same ones");
    }

    /// Every way of changing an earlier item moves the count down to it, and
    /// only that item is copied.
    #[test]
    fn every_change_below_the_end_is_counted() {
        let earlier: Vec<Arc<TranscriptItem>> = ["a", "b", "c", "d"].map(|text| Arc::new(item(text))).into();

        let mut log = ItemLog::resume(earlier.clone());
        log.get_mut(1).expect("there").text = "B".into();
        assert_eq!(log.changed_from(), 1);
        assert!(Arc::ptr_eq(&log.items()[0], &earlier[0]) && Arc::ptr_eq(&log.items()[2], &earlier[2]));
        assert_eq!(earlier[1].text, "b", "the earlier parse's item is left as it was");

        let mut log = ItemLog::resume(earlier.clone());
        log.last_mut().expect("there").text = "D".into();
        log.push(item("e"));
        assert_eq!((texts(&log), log.changed_from()), (vec!["a", "b", "c", "D", "e"], 3));

        let mut log = ItemLog::resume(earlier.clone());
        log.replace(2, item("C"));
        assert_eq!((texts(&log), log.changed_from(), log.moved_from()), (vec!["a", "b", "C", "d"], 2, 4), "replaced in place: nothing moved");

        let mut log = ItemLog::resume(earlier);
        log.annotate_mut(0).text = "chips".into();
        assert_eq!(log.changed_from(), 4, "the chips are not the transcript's");
    }

    fn tool(text: &str) -> TranscriptItem {
        make_item(Role::Assistant, ItemKind::ToolUse, text, None)
    }

    /// Taking an item out or putting some in counts as a change from there,
    /// and as a move; and whether an item the annotation could chip was
    /// taken out or moved along is kept.
    #[test]
    fn taking_out_and_putting_in_count_as_moves() {
        let earlier: Vec<Arc<TranscriptItem>> = vec![Arc::new(item("said")), Arc::new(tool("call")), Arc::new(tool("cell"))];

        let mut log = ItemLog::resume(earlier.clone());
        assert_eq!(log.pop().map(|item| item.text), Some("cell".into()));
        log.push(tool("ran"));
        assert_eq!((texts(&log), log.changed_from(), log.moved_from()), (vec!["said", "call", "ran"], 2, 2));
        assert!(!log.moved_annotatable(), "a tool call is not chipped");

        let mut log = ItemLog::resume(earlier.clone());
        log.insert(2, [tool("x"), item("y")]);
        assert_eq!((texts(&log), log.changed_from(), log.moved_from()), (vec!["said", "call", "x", "y", "cell"], 2, 2));
        assert!(!log.moved_annotatable(), "only the cell's row moved along; what was put in is new");

        let mut log = ItemLog::resume(earlier.clone());
        log.insert(0, [tool("first")]);
        assert!(log.moved_annotatable(), "the message moved along");

        let mut log = ItemLog::resume(vec![Arc::new(tool("call")), Arc::new(item("said"))]);
        log.pop();
        assert!(log.moved_annotatable(), "the message taken out");
        assert_eq!(ItemLog::resume(earlier).moved_from(), 3, "nothing moved");
    }
}
