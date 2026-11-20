//! A transcript's items as a parse makes them, and how far back it changed
//! them.
//!
//! A parse appends items as it reads entries, and now and then changes one it
//! made earlier: a question its answer settles, a command its output arrives
//! for, a subagent's launch once its sidecar is on disk. A parse that goes on
//! from an earlier one ([`crate::resume`]) starts from that parse's items and
//! has to say which of them it changed, so that the fingerprint chain is made
//! again from there and not from the start ([`crate::loader`]). [`ItemLog`]
//! keeps that count itself: the only ways to change an item it holds are
//! [`ItemLog::get_mut`] and [`ItemLog::last_mut`], and both count.
//!
//! What it cannot check is the parser's side of it. A parse that went on is
//! only right when the parser's own state (what it remembers of the entries
//! before) leads it to make exactly the changes a parse from the start makes;
//! that is what the parser's own tests prove against such a parse. Nor does it
//! move or remove items: a parser that needs to should add those here with a
//! count of their own, since the chips ([`crate::mentions::Mentions`]) then
//! have to be walked again from there.
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

        let mut log = ItemLog::resume(earlier);
        log.annotate_mut(0).text = "chips".into();
        assert_eq!(log.changed_from(), 4, "the chips are not the transcript's");
    }
}
