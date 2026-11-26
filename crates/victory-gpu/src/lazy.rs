//! Lazy read-back (`VICTORY_LAZY_BACK=1`; off unless asked for): what
//! shaders wrote into a copy in the PC's memory goes into title memory not
//! when the submission lands but when something touches that memory.
//!
//! On the console the CPU reads what the GPU wrote from the one memory
//! both have. Here every byte shaders write goes back into title memory
//! after each submission (`crate::land`), for the CPU to find it there:
//! on this PC 40 MB a frame, of which the title's own code then read 0.2 %
//! (the read census, `crate::readwatch`, run 6: three windows of 40 frames
//! each, 98-99 % of the landed pages in copies the title never touched).
//! Of the relay thread's 30-36 ms a frame, 26 were the compare of copies in
//! the PC's memory with their snapshots, and the submitting thread spent 15
//! more taking those snapshots.
//!
//! So for such a copy ([`crate::shadow::Shadow::lazy_back`]: in the PC's
//! memory, the title's own allocation, not a window) a landing does not
//! compare or merge. It notes the declared ranges' 64 KiB pieces as
//! **stale** (title memory there is behind the copy), makes their pages of
//! title memory no-access, and keeps the snapshot. The first touch of a
//! stale page, by whoever, faults into the handler of `crate::watch`
//! ([`fault`]), which lands the piece: what differs between the copy and the
//! snapshot goes into title memory, as the eager landing would have put it
//! ([`crate::land::piece`]), the pages are given back, and the instruction
//! runs again. A kernel write into title memory, which does not fault,
//! lands what it is about to write first ([`lift`], from
//! `crate::watch::writing` and `kernel_writing`). A piece a kernel write is
//! under way in is landed at once instead of guarded ([`active`]).
//!
//! A stale piece stays so until touched, through any number of further
//! submissions that write it: the snapshot of a stale piece is not taken
//! again (`Shadow::writing` leaves stale pieces out), so the compare at the
//! touch finds everything the card wrote since. A copy whose pages the
//! title touches often is of no use to this and goes back to eager landing
//! ([`State::eager`]).
//!
//! What this does not cover yet: copies in the card's memory (their pieces
//! need a fetch from the card at the touch), and copies of views of the
//! title's physical pages (their protection belongs to `crate::watch`).
#![cfg(windows)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

use crate::shadow::Shadow;

pub(crate) const PAGE: usize = 4096;
/// The unit pieces are cut in, of title memory (`crate::shadow`'s).
pub(crate) const PIECE: usize = 64 << 10;

/// See the file.
pub(crate) fn on() -> bool {
    mode() != 0
}

/// `VICTORY_LAZY_BACK`: 0 (unset) off; 1 lazy; 2 verifying: the pieces are
/// landed at once all the same, with the protections and the bookkeeping of
/// the lazy case, so that a fault of the title's tells the machinery from
/// the deferral; 3 protecting only: the eager landing as before, and only
/// the protections and the handler on top (a fault gives the pages back and
/// lands nothing), which tells the protections from the landing's changes.
pub(crate) fn mode() -> u8 {
    static MODE: OnceLock<u8> = OnceLock::new();
    *MODE.get_or_init(|| {
        let mode = match std::env::var("VICTORY_LAZY_BACK").as_deref().map(str::trim) {
            Ok("1") => 1,
            Ok("2") => 2,
            Ok("3") => 3,
            _ => 0,
        };
        if mode != 0 && !(crate::shadow::relay() && crate::land::on()) {
            tracing::warn!("VICTORY_LAZY_BACK needs the relay (VICTORY_RELAY) and the landing threads (VICTORY_LAND): off");
            return 0;
        }
        mode
    })
}

pub(crate) fn verify() -> bool {
    mode() == 2
}

pub(crate) fn protect_only() -> bool {
    mode() == 3
}

/// `VICTORY_LAZY_TRACE=1`: the first [`TRACE_MOST`] events (deferrals,
/// faults, lifts, commits) go to the log, for a crash to be read against.
pub(crate) fn trace(what: std::fmt::Arguments<'_>) {
    static LEFT: AtomicU64 = AtomicU64::new(TRACE_MOST);
    if !tracing_on() {
        return;
    }
    if LEFT.fetch_sub(1, Relaxed) > 0 {
        tracing::info!("lazy trace: {what}");
    }
}
const TRACE_MOST: u64 = 2000;

/// Where the instruction at `site` is, for the trace.
pub(crate) fn place(site: usize) -> String {
    crate::titlewrite::place(site).map_or_else(|| format!("{site:#x}"), |(i, o)| format!("{i}+{o:#x}"))
}

/// Touches of the title's between two landings of a copy past which it
/// lands eagerly again (`VICTORY_LAZY_TOUCHES`, 64).
fn touches_most() -> u64 {
    static MOST: OnceLock<u64> = OnceLock::new();
    *MOST.get_or_init(|| std::env::var("VICTORY_LAZY_TOUCHES").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(64))
}

/// What is stale in a copy: a bit per 64 KiB piece, and the offsets
/// themselves (the declared ranges not landed, in order and joined). Only
/// those offsets are compared and put at a landing: the snapshot holds the
/// copy's bytes from before only there, so a whole piece compared against
/// it would take the title's own bytes outside them for the card's and put
/// the copy's older ones over them (runs 7 to 10: the title's pointers put
/// back to old values, a null write while the world loaded).
#[derive(Default)]
pub(crate) struct Stale {
    pub(crate) bits: Vec<u64>,
    pub(crate) ranges: Vec<(usize, usize)>,
}

impl Stale {
    pub(crate) fn has(&self, piece: usize) -> bool {
        self.bits.get(piece / 64).is_some_and(|w| w >> (piece % 64) & 1 != 0)
    }

    pub(crate) fn set(&mut self, piece: usize) {
        self.bits[piece / 64] |= 1 << (piece % 64);
    }

    pub(crate) fn clear(&mut self, piece: usize) {
        self.bits[piece / 64] &= !(1 << (piece % 64));
    }

    pub(crate) fn none(&self) -> bool {
        self.bits.iter().all(|w| *w == 0)
    }

    /// Adds `[from, to)` to the ranges, joined with what touches it.
    pub(crate) fn add(&mut self, from: usize, to: usize) {
        if from >= to {
            return;
        }
        let i = self.ranges.partition_point(|r| r.1 < from);
        let mut j = i;
        let (mut a, mut b) = (from, to);
        while j < self.ranges.len() && self.ranges[j].0 <= to {
            a = a.min(self.ranges[j].0);
            b = b.max(self.ranges[j].1);
            j += 1;
        }
        self.ranges.splice(i..j, [(a, b)]);
    }

    /// Takes `[from, to)` out of the ranges.
    pub(crate) fn remove(&mut self, from: usize, to: usize) {
        let mut out = Vec::with_capacity(self.ranges.len() + 1);
        for &(a, b) in &self.ranges {
            if b <= from || a >= to {
                out.push((a, b));
                continue;
            }
            if a < from {
                out.push((a, from));
            }
            if to < b {
                out.push((to, b));
            }
        }
        self.ranges = out;
    }

    /// The parts of the ranges inside `[lo, hi)`, as offsets from `lo`.
    pub(crate) fn parts(&self, lo: usize, hi: usize) -> Vec<(usize, usize)> {
        let i = self.ranges.partition_point(|r| r.1 <= lo);
        self.ranges[i..]
            .iter()
            .take_while(|r| r.0 < hi)
            .map(|&(a, b)| (a.max(lo) - lo, b.min(hi) - lo))
            .filter(|p| p.0 < p.1)
            .collect()
    }
}

/// What a copy keeps for this.
#[derive(Default)]
pub(crate) struct State {
    pub(crate) stale: Mutex<Stale>,
    /// Whether any bit is set (the quick look).
    pub(crate) any: AtomicBool,
    /// The title touched its stale pages too often: landed eagerly from now
    /// on.
    pub(crate) eager: AtomicBool,
    /// Touches of the title's since the last landing (halved at each).
    pub(crate) touches: AtomicU64,
    /// In [`registry`].
    pub(crate) registered: AtomicBool,
    /// Whether the memory is the title's own ([`private`]): 0 not asked
    /// yet, 1 yes, 2 no.
    pub(crate) private: std::sync::atomic::AtomicU8,
    /// Whether no other copy holds any of this memory (`Shadow::alone`):
    /// the count of buffer changes it was asked at, shifted, and the answer.
    pub(crate) alone: AtomicU64,
    /// Where the snapshot's bytes are while any piece is stale (the
    /// snapshot stays until then, `Shadow::copy_back_done`), so that a
    /// landing in the handler needs no lock: the thread that faulted may
    /// hold the snapshot's.
    pub(crate) snapshot: std::sync::atomic::AtomicPtr<u8>,
}

thread_local! {
    /// Whether this thread is inside a landing of ours ([`Landing`]): its
    /// merge writes title memory, which tells the guards (`watch::landing`
    /// -> `writing` -> [`lift`]), which would land again what it is landing,
    /// on the lock it holds.
    static LANDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Set while a thread lands ([`landing`]).
pub(crate) struct Landing(bool);

pub(crate) fn landing() -> Landing {
    Landing(LANDING.replace(true))
}

impl Drop for Landing {
    fn drop(&mut self) {
        LANDING.set(self.0);
    }
}

pub(crate) fn is_landing() -> bool {
    LANDING.get()
}

impl State {
    pub(crate) fn lock(&self) -> MutexGuard<'_, Stale> {
        self.stale.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The lock, unless this thread is landing and the lock is its own: then
    /// `None`. Inside a landing the lock cannot be waited for outright (it
    /// may be this thread's), but another thread holds it only for
    /// microseconds (a deferral, a landing of one piece), so it is tried for
    /// a while first: giving up at once left a neighbour's piece stale on a
    /// page just given back (run 17).
    pub(crate) fn lock_unless_landing(&self) -> Option<MutexGuard<'_, Stale>> {
        if !is_landing() {
            return Some(self.lock());
        }
        let started = std::time::Instant::now();
        loop {
            if let Ok(guard) = self.stale.try_lock() {
                return Some(guard);
            }
            if started.elapsed() > std::time::Duration::from_millis(2) {
                STATS[GAVE_UP].fetch_add(1, Relaxed);
                return None;
            }
            std::thread::yield_now();
        }
    }

    /// The title touched a stale page: whether the copy should land eagerly
    /// from now on is decided at its next landing ([`State::settle`]).
    pub(crate) fn touched(&self) {
        self.touches.fetch_add(1, Relaxed);
    }

    /// At a landing: whether the copy is still worth deferring.
    pub(crate) fn settle(&self) -> bool {
        let touches = self.touches.load(Relaxed);
        self.touches.store(touches / 2, Relaxed);
        if touches > touches_most() && !self.eager.swap(true, Relaxed) {
            STATS[EAGER_COPIES].fetch_add(1, Relaxed);
        }
        !self.eager.load(Relaxed)
    }
}

/// The 64 KiB blocks of title memory some copy has a stale piece in: a
/// filter for [`lift`], which is called often. Set before the pages are
/// protected; a block may stay set a while after its last piece landed.
static STALE: crate::watch::Bits = crate::watch::Bits::new();
/// Whether any copy ever deferred a landing (the handler's first look;
/// never cleared: the handler then asks the copies themselves).
static ANY: AtomicBool = AtomicBool::new(false);
/// The copies that ever deferred a landing.
static REGISTRY: Mutex<Vec<Weak<Shadow>>> = Mutex::new(Vec::new());
/// Title memory kernel writes are under way in (address, end).
static ACTIVE: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

fn registry() -> MutexGuard<'static, Vec<Weak<Shadow>>> {
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
}

/// A copy deferred a landing: known to the handler from now on.
pub(crate) fn register(shadow: &Arc<Shadow>) {
    if shadow.lazy.registered.swap(true, Relaxed) {
        return;
    }
    let mut r = registry();
    r.retain(|w| w.strong_count() > 0);
    r.push(Arc::downgrade(shadow));
    ANY.store(true, Relaxed);
    STATS[COPIES].fetch_add(1, Relaxed);
}

/// The live copies whose pages (whole pages of 4 KiB) may hold `[at, end)`.
fn covering(at: usize, end: usize) -> Vec<Arc<Shadow>> {
    let r = registry();
    r.iter()
        .filter_map(Weak::upgrade)
        .filter(|s| {
            let (start, len) = s.span();
            let (lo, hi) = ((start as usize) / PAGE * PAGE, (start + len) as usize);
            lo < end && hi.next_multiple_of(PAGE) > at
        })
        .collect()
}

/// A block of title memory is stale in some copy.
pub(crate) fn block_stale(block: usize) {
    STALE.set(block);
}

/// A copy landed its stale pieces in `blocks`: the blocks no other copy has
/// a stale piece in are clear.
pub(crate) fn blocks_landed(blocks: &[usize]) {
    if blocks.is_empty() {
        return;
    }
    let others = covering(blocks[0] * PIECE, (blocks[blocks.len() - 1] + 1) * PIECE);
    for &block in blocks {
        if !others.iter().any(|s| s.has_stale_block(block)) {
            STALE.clear(block);
        }
    }
}

/// Pages `[lo, hi)` of title memory (page-aligned) made no-access. `false`
/// if that could not be done, or they were not plain read-write pages.
pub(crate) fn protect(lo: usize, hi: usize) -> bool {
    use windows_sys::Win32::System::Memory::{PAGE_NOACCESS, PAGE_READWRITE, VirtualProtect};
    // (No `VirtualQuery` here: on the title's big regions it walks the page
    // tables for milliseconds a call, which was 100-200 ms a frame in run
    // 11. Whether the memory is the title's own is asked once per copy,
    // [`private`]; the protection's own answer says the rest.)
    let started = std::time::Instant::now();
    let mut old = 0;
    // SAFETY: title memory of the title's own allocation; only the
    // protection changes, and the handler gives it back at the first touch.
    let done = unsafe { VirtualProtect(lo as *const _, hi - lo, PAGE_NOACCESS, &mut old) } != 0;
    // `old` is the first page's. No-access there is a neighbouring copy's
    // stale piece on a page both share: the run is protected now and both
    // claims stand (a landing of either lands the other's, `land_others`).
    // Anything else (a guard, executable pages) is not ours to change:
    // given back as it was, landed eagerly. (Refusing no-access as well
    // put the first page's protection on the whole run and dropped the
    // claim: pages no piece claimed, which a kernel write cannot fault
    // into and so fails on, and the title read what was there before: runs
    // 8 to 15.)
    if done && old != PAGE_READWRITE && old != PAGE_NOACCESS {
        // SAFETY: as above.
        unsafe { VirtualProtect(lo as *const _, hi - lo, old, &mut old) };
        STATS[ODD_PAGES].fetch_add(((hi - lo) / PAGE) as u64, Relaxed);
        STATS[PROTECT_US].fetch_add(started.elapsed().as_micros() as u64, Relaxed);
        return false;
    }
    if !done {
        STATS[REFUSED].fetch_add(1, Relaxed);
    }
    STATS[PROTECT_US].fetch_add(started.elapsed().as_micros() as u64, Relaxed);
    done
}

/// Whether the title memory at `at` is the title's own committed private
/// memory (not a view of its physical pages, which are shown at other
/// addresses too, where no protection of this one would be seen). Asked
/// once per copy: `VirtualQuery` is slow on the title's regions.
pub(crate) fn private(state: &State, at: usize) -> bool {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, VirtualQuery};
    match state.private.load(Relaxed) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a question about an address, with room for the answer.
    let n = unsafe { VirtualQuery(at as *const _, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
    let yes = n != 0 && info.State == MEM_COMMIT && info.Type == MEM_PRIVATE;
    state.private.store(if yes { 1 } else { 2 }, Relaxed);
    yes
}

/// Copies smaller than this are landed eagerly (`VICTORY_LAZY_MIN_KB`,
/// 256): a counter of four bytes shaders write and the title polls would
/// fault at every poll, and the census's gain was in copies of megabytes.
pub(crate) fn min_bytes() -> u64 {
    static MIN: OnceLock<u64> = OnceLock::new();
    *MIN.get_or_init(|| std::env::var("VICTORY_LAZY_MIN_KB").ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(256) << 10)
}

/// Only copies no other copy overlaps defer (on; `VICTORY_LAZY_ALONE=0`:
/// every copy). With the deferred pages noted in the journal the other
/// copies of the memory refresh, fault and land them all the same: run 17,
/// 637 pieces a frame deferred and 735 landed within the frame, 703 of them
/// at faults of our own copies, 900 ms a frame in the handler. The title's
/// buffers lie inside bigger ones, each with a copy; what one copy defers
/// the others need the same frame. So deferring pays only where a copy is
/// alone with its memory, about one per cent of the bytes landed here.
pub(crate) fn alone_only() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_LAZY_ALONE").is_none_or(|v| v != "0"))
}

/// Whether the trace is on (so that what it says is only made then).
pub(crate) fn tracing_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_LAZY_TRACE").is_some_and(|v| v == "1"))
}

/// Pages `[lo, hi)` given back (read-write).
pub(crate) fn unprotect(lo: usize, hi: usize) {
    use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
    let started = std::time::Instant::now();
    let mut old = 0;
    // SAFETY: pages this module made no-access.
    if unsafe { VirtualProtect(lo as *const _, hi - lo, PAGE_READWRITE, &mut old) } == 0 {
        STATS[REFUSED].fetch_add(1, Relaxed);
    }
    STATS[PROTECT_US].fetch_add(started.elapsed().as_micros() as u64, Relaxed);
}

/// What kernel writes are under way in, now.
pub(crate) fn active() -> Vec<(usize, usize)> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A kernel write into `[at, at + len)` begins: nothing there is guarded
/// until [`active_end`].
pub(crate) fn active_begin(at: usize, len: usize) {
    if len != 0 && on() {
        ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).push((at, at + len));
    }
}

pub(crate) fn active_end(at: usize, len: usize) {
    if len != 0 && on() {
        let mut a = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = a.iter().position(|&r| r == (at, at + len)) {
            a.swap_remove(i);
        }
    }
}

/// Title memory `[at, at + len)` is about to be written by the kernel, or
/// read by something that cannot fault: its stale pieces are landed.
pub(crate) fn lift(at: usize, len: usize) {
    lift_for("write", at, len);
}

/// [`lift`], with `why` for the trace: who is about to touch the memory.
pub(crate) fn lift_for(why: &str, at: usize, len: usize) {
    // (From inside a landing of ours: what it writes is what it lands.)
    if len == 0 || !ANY.load(Relaxed) || is_landing() {
        return;
    }
    let end = at.saturating_add(len);
    if !STALE.any(at / PIECE, end.div_ceil(PIECE)) {
        return;
    }
    let landed: u64 = covering(at, end).iter().map(|s| s.land_stale(at, end)).sum();
    if landed > 0 || tracing_on() {
        trace(format_args!("lift for {why} {at:#x}+{len:#x}: {landed} bytes landed"));
    }
    STATS[LIFTED].fetch_add(landed, Relaxed);
}

/// The title committed (or committed again) `[at, at + len)`: Windows gives
/// pages already committed the protection asked for, so what this module
/// made no-access there is read-write again with its pieces still noted
/// stale. They are landed now, before the title touches the memory, as for
/// a kernel write.
pub(crate) fn committed(at: usize, len: usize) {
    if len == 0 || !ANY.load(Relaxed) {
        return;
    }
    let end = at.saturating_add(len);
    if STALE.any(at / PIECE, end.div_ceil(PIECE)) {
        STATS[COMMITS].fetch_add(1, Relaxed);
        lift_for("commit", at, len);
    }
}

/// Title memory `[at, at + len)` (with no length: the allocation at `at`,
/// taken as all that follows) is about to be freed: what is stale there is
/// landed first, as for a kernel write. Simply forgetting it left no-access
/// pages behind when the memory stayed (run 8), and landing into memory
/// about to go costs little.
pub(crate) fn freed(at: usize, len: usize) {
    if !ANY.load(Relaxed) {
        return;
    }
    lift_for("free", at, if len == 0 { usize::MAX - at } else { len });
}

/// A copy gave pages `[lo, hi)` back: the other copies with a stale piece
/// on them land it too, since a page is read-write for all or none.
pub(crate) fn land_others(this: *const Shadow, lo: usize, hi: usize) {
    for s in covering(lo, hi) {
        if !std::ptr::eq(Arc::as_ptr(&s), this) {
            STATS[NEIGHBOURS].fetch_add(s.land_stale(lo, hi), Relaxed);
        }
    }
}

/// Copy `this` is about to ask the write watch about title memory
/// `[at, at + len)`, to bring its own bytes up to date: what OTHER copies
/// left stale there is landed first, so that the asking finds it written
/// (the eager landing's writes set the kernel's bits, which is how one copy
/// learns of the card's writes through another; `this` own stale pieces
/// are its own business and stay).
pub(crate) fn lift_except(this: *const Shadow, at: usize, len: usize) {
    if len == 0 || !ANY.load(Relaxed) || is_landing() {
        return;
    }
    let end = at.saturating_add(len);
    if !STALE.any(at / PIECE, end.div_ceil(PIECE)) {
        return;
    }
    for s in covering(at, end) {
        if !std::ptr::eq(Arc::as_ptr(&s), this) {
            STATS[LIFTED].fetch_add(s.land_stale(at, end), Relaxed);
        }
    }
}

/// From the exception handler: an access violation at `at` by the
/// instruction at `site`, `ours` if in one of our copies. True if the page
/// was stale in some copy: it is landed and the instruction may run again.
pub(crate) fn fault(at: usize, site: usize, ours: bool) -> bool {
    if !ANY.load(Relaxed) {
        return false;
    }
    let started = std::time::Instant::now();
    let page = at / PAGE * PAGE;
    // (Not by the blocks' bits: a copy notes its pieces before it protects
    // their pages, but a block's bit may be cleared by another copy's
    // landing between the two.)
    let shadows = covering(page, page + PAGE);
    if shadows.is_empty() {
        return false;
    }
    let mut landed = 0;
    for s in &shadows {
        landed += s.land_stale(page, page + PAGE);
        if !ours {
            s.lazy.touched();
        }
    }
    if tracing_on() {
        trace(format_args!(
            "fault at {at:#x} by {} ({}): {} copies cover, {landed} bytes landed",
            place(site),
            if ours { "ours" } else { "title" },
            shadows.len(),
        ));
    }
    if landed == 0 && !crate::watch::writable(at) {
        // A page of a copy's that is not read-write and that no piece
        // claims: not ours, unless it is no-access, which only this module
        // makes title memory. Then the bookkeeping and the pages disagree,
        // and the page is given back rather than faulted for ever.
        if !no_access(at) {
            return false;
        }
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::warn!(at = format_args!("{at:#x}"), "lazy read-back: a no-access page no piece claims, given back"));
        STATS[ORPHANS].fetch_add(1, Relaxed);
        unprotect(page, page + PAGE);
    }
    STATS[if ours { FAULTS_OURS } else { FAULTS_TITLE }].fetch_add(1, Relaxed);
    STATS[FAULT_US].fetch_add(started.elapsed().as_micros() as u64, Relaxed);
    true
}

/// Whether the page at `at` is committed and no-access.
fn no_access(at: usize) -> bool {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_NOACCESS, VirtualQuery};
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a question about an address, with room for the answer.
    let n = unsafe { VirtualQuery(at as *const _, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
    n != 0 && info.State == MEM_COMMIT && info.Protect == PAGE_NOACCESS
}

/// Since the last frame-rate line.
pub(crate) const DEFERRED_PIECES: usize = 0;
pub(crate) const DEFERRED_BYTES: usize = 1;
pub(crate) const LANDED_PIECES: usize = 2;
pub(crate) const LANDED_BYTES: usize = 3;
pub(crate) const LAND_US: usize = 4;
pub(crate) const FAULTS_TITLE: usize = 5;
pub(crate) const FAULTS_OURS: usize = 6;
pub(crate) const FAULT_US: usize = 7;
pub(crate) const LIFTED: usize = 8;
pub(crate) const UNDER_WRITE: usize = 9;
pub(crate) const PROTECT_US: usize = 10;
pub(crate) const REFUSED: usize = 11;
pub(crate) const ODD_PAGES: usize = 12;
pub(crate) const COPIES: usize = 13;
pub(crate) const EAGER_COPIES: usize = 14;
pub(crate) const NO_SNAPSHOT: usize = 15;
pub(crate) const ORPHANS: usize = 16;
pub(crate) const NEIGHBOURS: usize = 17;
pub(crate) const SYNCED: usize = 18;
pub(crate) const COMMITS: usize = 19;
pub(crate) const NOT_ALONE: usize = 20;
pub(crate) const GAVE_UP: usize = 21;
const COUNT: usize = 22;
pub(crate) static STATS: [AtomicU64; COUNT] = [const { AtomicU64::new(0) }; COUNT];

/// For the frame-rate line.
pub(crate) fn log(frames: f64) {
    if !on() {
        return;
    }
    let s: Vec<f64> = STATS.iter().map(|r| r.swap(0, Relaxed) as f64).collect();
    // (Counts since the start, not since the last line.)
    for i in [COPIES, EAGER_COPIES] {
        STATS[i].fetch_add(s[i] as u64, Relaxed);
    }
    let per = |i: usize| s[i] / frames;
    if s[DEFERRED_PIECES] == 0.0 && s[LANDED_PIECES] == 0.0 {
        return;
    }
    tracing::info!(
        "lazy read-back per frame (VICTORY_LAZY_BACK=1): {:.0} pieces deferred ({:.1} MB not compared or merged), {:.1} landed at a touch \
         ({:.2} MB, {:.2} ms); touched first by the title {:.1} times, by our copies {:.1} ({:.2} ms in the handler), landed ahead of a \
         kernel write {:.1}, landed at once under one {:.1}; protections {:.2} ms, {:.0} refused, {:.0} pages not read-write; \
         {} copies deferring so far, {} gone back to eager, {} landings with no snapshot, {:.0} pages given back unclaimed, \
         {:.1} bytes landed for a neighbour, {:.2} MB of refreshed bytes kept in snapshots, {:.0} commits over stale pieces, \
         {:.0} landings kept eager for another copy of the memory, {:.0} locks given up on inside a landing{}",
        per(DEFERRED_PIECES),
        per(DEFERRED_BYTES) / 1e6,
        per(LANDED_PIECES),
        per(LANDED_BYTES) / 1e6,
        per(LAND_US) / 1000.0,
        per(FAULTS_TITLE),
        per(FAULTS_OURS),
        per(FAULT_US) / 1000.0,
        per(LIFTED),
        per(UNDER_WRITE),
        per(PROTECT_US) / 1000.0,
        s[REFUSED],
        s[ODD_PAGES],
        s[COPIES],
        s[EAGER_COPIES],
        s[NO_SNAPSHOT],
        s[ORPHANS],
        per(NEIGHBOURS),
        per(SYNCED) / 1e6,
        s[COMMITS],
        per(NOT_ALONE),
        s[GAVE_UP],
        match mode() {
            2 => " (VICTORY_LAZY_BACK=2: landed at once all the same)",
            3 => " (VICTORY_LAZY_BACK=3: protections only, the landing eager)",
            _ => "",
        },
    );
}
