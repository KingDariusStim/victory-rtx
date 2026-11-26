//! The per-draw ("dynamic") view slots of a host heap.
//!
//! A draw's descriptor table gets a run of consecutive slots as long as its
//! shader's highest table offset plus one, given back when the list's
//! submission has run. On a host whose view heaps hold 1,000,000 views
//! (NVIDIA) the title's slots leave 262,144 for these, and the world asks
//! for 10,000-12,000 tables a frame, some 200,000 slots (run 27). Every
//! refused table is a draw not drawn (the tiled lighting's rectangles, the
//! mesh dispatches of the world's geometry).
//!
//! What served them, and how it did (runs 23-25, the cell and the yard):
//! - a ring with a bit per slot and a first-fit search from a cursor:
//!   16.8% of the tables refused, and 149 ms a frame of searching once the
//!   area filled (every refused table walked the ring);
//! - size classes on pages of 256 slots (runs 24-25): 6-11 ms, but 27.8%
//!   refused: lengths rounded up to their class, and pages held by a class
//!   while partly used, so the area filled sooner.
//!
//! - the shortest free stretch that holds a table (best fit, run 30): 23.0%,
//!   the area cut into crumbs (51,810 slots free in 7,718 stretches, the
//!   longest 102).
//!
//! The ring placed tables in the order they come, and they are given back
//! in about that order (a list's tables when its submission has run), so
//! what is given back behind its cursor joins into long stretches: the
//! fewest refused. [`Extents`] does what the ring did over free stretches,
//! not slots: from the cursor on, the first free stretch that holds the
//! table (next fit), exact; one given back joins the free stretches beside
//! it. The longest free stretch is known at once (an index by length), so a
//! table no stretch holds is refused without a search, the ring's 149 ms.

use std::collections::{BTreeMap, BTreeSet};

/// The sparse zone's size is whole multiples of this.
pub(crate) const PAGE: u32 = 256;

/// The free stretches of the dynamic slots.
pub(crate) struct Extents {
    /// Start → length.
    by_start: BTreeMap<u32, u32>,
    /// (Length, start): the longest, to refuse without a search.
    by_length: BTreeSet<(u32, u32)>,
    slots: u32,
    free: u32,
    /// Where the next table's search starts: past the last one placed.
    cursor: u32,
}

impl Extents {
    pub(crate) fn new(slots: u32) -> Extents {
        let mut e = Extents { by_start: BTreeMap::new(), by_length: BTreeSet::new(), slots, free: 0, cursor: 0 };
        if slots > 0 {
            e.insert(0, slots);
        }
        e
    }

    fn insert(&mut self, start: u32, len: u32) {
        self.by_start.insert(start, len);
        self.by_length.insert((len, start));
        self.free += len;
    }

    fn remove(&mut self, start: u32, len: u32) {
        self.by_start.remove(&start);
        self.by_length.remove(&(len, start));
        self.free -= len;
    }

    /// Slots free now.
    pub(crate) fn free_slots(&self) -> u32 {
        self.free
    }

    /// Free stretches, and the longest.
    pub(crate) fn census(&self) -> (u32, u32) {
        (self.by_start.len() as u32, self.by_length.last().map_or(0, |&(len, _)| len))
    }

    /// `n` consecutive slots: the first, or `None` when no free stretch
    /// holds them.
    pub(crate) fn take(&mut self, n: u32) -> Option<u32> {
        if n == 0 || self.by_length.last().is_none_or(|&(len, _)| len < n) {
            return None;
        }
        // From the cursor on (a stretch the cursor lies in counts from the
        // cursor), then from the start: the first that holds `n`.
        let cursor = self.cursor;
        let inside = self
            .by_start
            .range(..cursor)
            .next_back()
            .filter(|&(&s, &l)| s + l >= cursor + n)
            .map(|(&s, &l)| (s, l));
        // At most `NEAR` stretches on from the cursor, then the shortest
        // anywhere that holds it: a walk over every crumb near the cursor
        // took 50 ms a frame (run 31, 4,000-5,000 stretches).
        const NEAR: usize = 64;
        let found = inside
            .map(|(s, l)| (s, l, cursor))
            .or_else(|| {
                self.by_start
                    .range(cursor..)
                    .chain(self.by_start.range(..cursor))
                    .take(NEAR)
                    .find(|&(_, &l)| l >= n)
                    .map(|(&s, &l)| (s, l, s))
            })
            .or_else(|| self.by_length.range((n, 0)..).next().map(|&(l, s)| (s, l, s)));
        let (start, len, at) = found?;
        self.remove(start, len);
        if at > start {
            self.insert(start, at - start);
        }
        let end = start + len;
        if end > at + n {
            self.insert(at + n, end - at - n);
        }
        self.cursor = if at + n >= self.slots { 0 } else { at + n };
        Some(at)
    }

    /// The `n` slots from `first` given back. Slots not all taken (given
    /// back twice, or never handed out) are left alone.
    pub(crate) fn give(&mut self, first: u32, n: u32) {
        let Some(end) = first.checked_add(n).filter(|&e| n > 0 && e <= self.slots) else {
            return;
        };
        // The free stretch before: it must end at or before `first`.
        let before = self.by_start.range(..=first).next_back().map(|(&s, &l)| (s, l));
        if before.is_some_and(|(s, l)| s + l > first) {
            return;
        }
        // The one after: it must start at or after `end`.
        let after = self.by_start.range(first..).next().map(|(&s, &l)| (s, l));
        if after.is_some_and(|(s, _)| s < end) {
            return;
        }
        let (mut start, mut len) = (first, n);
        if let Some((s, l)) = before.filter(|&(s, l)| s + l == first) {
            self.remove(s, l);
            start = s;
            len += l;
        }
        if let Some((s, l)) = after.filter(|&(s, _)| s == end) {
            self.remove(s, l);
            len += l;
        }
        self.insert(start, len);
    }

    /// Whether `slot` is taken (for the books); `None` past the end.
    pub(crate) fn holds(&self, slot: u32) -> Option<bool> {
        if slot >= self.slots {
            return None;
        }
        let free = self.by_start.range(..=slot).next_back().is_some_and(|(&s, &l)| slot < s + l);
        Some(!free)
    }
}

/// The long tables' slots: only the offsets their readers name.
///
/// A translated shader reads a table's entry at the heap index of the
/// table's start plus the entry's offset (`victory_shader` `heap_index`),
/// and nothing between the entries its program lists (and the ones picked
/// by index, known at the draw). The long tables of this title are mostly
/// holes: in run 28, 60 kinds of table over 64 slots long read 1,187 of
/// their 9,182 slots, 13% (a table of 101 slots whose shader reads one, the
/// 101st). Here a table is placed where each offset it reads finds a free
/// slot, and the holes between hold other tables' entries: tables interlock.
/// The table's start may lie below the zone (`base` plus its lowest offset
/// is in it), its holes on anything.
pub(crate) struct Sparse {
    /// A bit per slot of the zone, set while taken.
    taken: Vec<u64>,
    slots: u32,
    /// Where the next search starts.
    cursor: u32,
    /// What each table placed took: its start (zone slots from the zone's
    /// first, plus `below`) and offsets.
    placed: std::collections::HashMap<u32, Box<[u32]>>,
    /// Slots taken now.
    used: u32,
}

/// How far below the zone a table's start may lie: its lowest offset read,
/// up to this.
pub(crate) const BELOW: u32 = 4096;

impl Sparse {
    pub(crate) fn new(slots: u32) -> Sparse {
        Sparse { taken: vec![0; slots.div_ceil(64) as usize], slots, cursor: 0, placed: Default::default(), used: 0 }
    }

    pub(crate) fn slots(&self) -> u32 {
        self.slots
    }

    pub(crate) fn free_slots(&self) -> u32 {
        self.slots - self.used
    }

    fn is_taken(&self, at: u32) -> bool {
        self.taken[(at / 64) as usize] & (1 << (at % 64)) != 0
    }

    fn set(&mut self, at: u32, taken: bool) {
        let bit = 1 << (at % 64);
        if taken {
            self.taken[(at / 64) as usize] |= bit;
        } else {
            self.taken[(at / 64) as usize] &= !bit;
        }
    }

    /// The next free slot at `from` or after it, if any before the end.
    fn next_free(&self, from: u32) -> Option<u32> {
        let mut word = (from / 64) as usize;
        let mut free = !self.taken.get(word)? & (u64::MAX << (from % 64));
        loop {
            if free != 0 {
                let at = word as u32 * 64 + free.trailing_zeros();
                return (at < self.slots).then_some(at);
            }
            word += 1;
            free = !*self.taken.get(word)?;
        }
    }

    /// A place for a table reading `reads` (sorted, distinct): its start,
    /// counted so that start plus an offset is the zone slot (plus
    /// [`BELOW`]); `None` when no place is found in one pass over the zone.
    pub(crate) fn take(&mut self, reads: &[u32]) -> Option<u32> {
        let (&low, &high) = (reads.first()?, reads.last()?);
        if low > BELOW || high - low >= self.slots {
            return None;
        }
        let span = high - low;
        let last_start = self.slots - 1 - span;
        // `at`: where the lowest offset goes. One pass, from the cursor
        // around to it.
        let mut at = self.cursor.min(last_start);
        let mut wrapped = false;
        loop {
            let Some(free) = self.next_free(at).filter(|&f| f <= last_start) else {
                if wrapped {
                    return None;
                }
                wrapped = true;
                at = 0;
                continue;
            };
            if wrapped && free >= self.cursor {
                return None;
            }
            if reads[1..].iter().all(|&k| !self.is_taken(free + k - low)) {
                for &k in reads {
                    self.set(free + k - low, true);
                }
                self.used += reads.len() as u32;
                self.cursor = free + 1;
                let start = free + BELOW - low;
                self.placed.insert(start, reads.into());
                return Some(start);
            }
            at = free + 1;
        }
    }

    /// The table placed at `start` (as [`Sparse::take`] returned it) gone.
    pub(crate) fn give(&mut self, start: u32) {
        let Some(reads) = self.placed.remove(&start) else {
            return;
        };
        for &k in reads.iter() {
            self.set(start + k - BELOW, false);
        }
        self.used -= reads.len() as u32;
    }

    /// Whether zone slot `at` is taken (for the books).
    pub(crate) fn holds(&self, at: u32) -> Option<bool> {
        (at < self.slots).then(|| self.is_taken(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tables whose reads interlock share a stretch of the zone; each slot
    /// read is one table's; all come back.
    #[test]
    fn sparse_tables_interlock_and_come_back() {
        let mut zone = Sparse::new(1024);
        let a = zone.take(&[0, 100]).unwrap();
        let b = zone.take(&[0, 100]).unwrap();
        let c = zone.take(&[5, 50, 99]).unwrap();
        // Slot of each read, in the zone.
        let at = |start: u32, k: u32| start + k - BELOW;
        assert_eq!((at(a, 0), at(a, 100)), (0, 100));
        assert_eq!((at(b, 0), at(b, 100)), (1, 101), "in the holes of the first");
        let all = [at(a, 0), at(a, 100), at(b, 0), at(b, 100), at(c, 5), at(c, 50), at(c, 99)];
        let mut seen = all.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), all.len(), "no slot read by two tables: {all:?}");
        assert_eq!(zone.free_slots(), 1024 - 7);
        for s in [a, b, c] {
            zone.give(s);
        }
        assert_eq!(zone.free_slots(), 1024);
        zone.give(a); // twice: nothing
        assert_eq!(zone.free_slots(), 1024);
    }

    /// A table whose start lies below the zone: its lowest offset is in it.
    #[test]
    fn a_sparse_start_may_lie_below_the_zone() {
        let mut zone = Sparse::new(64);
        let s = zone.take(&[100, 101]).unwrap();
        assert_eq!(s + 100, BELOW, "the read at 100 is the zone's first slot");
        assert!(zone.take(&[0, 64]).is_none(), "wider than the zone");
        assert!(zone.take(&[BELOW + 1]).is_none(), "start too far below");
    }

    /// Full: a table that finds no place is refused, after one pass, and
    /// one given back makes room again.
    #[test]
    fn a_full_zone_refuses_until_one_is_given_back() {
        let mut zone = Sparse::new(128);
        let starts: Vec<u32> = (0..64).map(|_| zone.take(&[0, 1]).unwrap()).collect();
        assert_eq!(zone.free_slots(), 0);
        assert!(zone.take(&[3]).is_none());
        zone.give(starts[10]);
        assert!(zone.take(&[0, 1]).is_some());
    }
    /// Exact lengths, next fit: a table goes past the last one placed, in
    /// the first free stretch that holds it, around to the start; one no
    /// stretch holds is refused.
    #[test]
    fn tables_go_past_the_last_placed_and_around() {
        let mut e = Extents::new(1000);
        let a = e.take(100).unwrap();
        let b = e.take(17).unwrap();
        let c = e.take(300).unwrap();
        assert_eq!((a, b, c), (0, 100, 117));
        assert_eq!(e.free_slots(), 1000 - 417);
        // A hole of 17 behind the cursor: the next table of 17 goes on
        // past the last placed, as the ring did.
        e.give(b, 17);
        assert_eq!(e.take(17), Some(417));
        assert_eq!(e.take(600), None, "the longest free stretch is 566");
        assert_eq!(e.take(566), Some(434), "to the end");
        assert_eq!(e.take(17), Some(100), "around to the start: the hole");
        assert_eq!(e.free_slots(), 0);
    }

    /// Given back, stretches join their free neighbours; the area is one
    /// stretch again.
    #[test]
    fn stretches_given_back_join_their_neighbours() {
        let mut e = Extents::new(64);
        let runs: Vec<u32> = (0..8).map(|_| e.take(8).unwrap()).collect();
        assert_eq!(e.take(1), None);
        for &r in runs.iter().step_by(2) {
            e.give(r, 8);
        }
        assert_eq!(e.census(), (4, 8));
        assert_eq!(e.take(9), None, "in pieces of 8");
        for &r in runs.iter().skip(1).step_by(2) {
            e.give(r, 8);
        }
        assert_eq!(e.census(), (1, 64));
        assert_eq!(e.take(64), Some(0));
    }

    /// A stretch given back twice, partly free already, or past the end, is
    /// left alone: two tables never share a slot.
    #[test]
    fn stray_gives_change_nothing() {
        let mut e = Extents::new(100);
        let a = e.take(10).unwrap();
        let b = e.take(10).unwrap();
        e.give(a, 10);
        e.give(a, 10);
        e.give(b + 5, 10); // half free already (the rest of the area)
        e.give(95, 10); // past the end
        e.give(b, 0);
        assert_eq!(e.free_slots(), 90);
        assert_eq!(e.holds(b), Some(true));
        assert_eq!(e.holds(a), Some(false));
        assert_eq!(e.take(10), Some(20), "past the last placed");
        assert_eq!(e.holds(25), Some(true));
    }

    /// Churn of a frame's tables, given back a frame later: the area does
    /// not fill with pieces (what the ring and the size classes did).
    #[test]
    fn churn_leaves_room_for_long_tables() {
        let mut e = Extents::new(64 * 256);
        let mut frames: std::collections::VecDeque<Vec<(u32, u32)>> = Default::default();
        for frame in 0..60u32 {
            let tables: Vec<(u32, u32)> = (0..300u32)
                .map(|i| {
                    let n = if i % 25 == 0 { 100 + (i + frame) % 150 } else { 1 + (i * 7 + frame) % 20 };
                    (e.take(n).expect("room"), n)
                })
                .collect();
            frames.push_back(tables);
            if frames.len() > 2 {
                for (first, n) in frames.pop_front().unwrap() {
                    e.give(first, n);
                }
            }
        }
        assert!(e.take(2000).is_some(), "a long stretch is left: {:?}", e.census());
    }
}
