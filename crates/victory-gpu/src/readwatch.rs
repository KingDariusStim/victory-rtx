//! Diagnostics: which of the pages a landing put shaders' bytes into the
//! title's own code then touches (`VICTORY_READ_WATCH=1`; builds with
//! diagnostics). The question behind lazy read-back: every frame the bytes
//! shaders wrote are fetched from the card and merged into title memory
//! (`crate::land`) before the title is told its work is done, but on the
//! console the CPU reads only some of that memory. What it never touches
//! need not be fetched at all.
//!
//! In the frames given (`VICTORY_READ_WATCH_AT`, seconds after the first
//! frame, `120,180,240` unless given; `VICTORY_READ_WATCH_FRAMES` frames each,
//! 20 unless given) the 4 KiB pages of title memory a landing changed are
//! made guard pages. The kernel takes the guard away at the first touch and
//! raises a guard-page violation, which [`fault`] notes (the first in the
//! process, `crate::watch::install`) and lets the instruction run again:
//! nothing waits and no protection is changed in the handler. For as many
//! frames again no new page is guarded, so that what was guarded last has as
//! long to be touched; then what is still guarded is let go and the log
//! says, per host copy, how many pages were guarded and by whom they were
//! touched first:
//!
//! - title: code outside our runtime, read or write, with the places it
//!   touched from most often;
//! - ours: our copies (`crate::safecopy`), mostly a refresh taking the page
//!   to the card again, or a later landing;
//! - kernel: a kernel write into it was about to be made ([`lift`], before a
//!   file read; the kernel does not fault into our handler, and a guard would
//!   have cut the read short);
//! - not at all.
//!
//! shortcut: only memory the title allocated itself (private, write-watched)
//! is guarded. Views of its physical pages belong to the guards of
//! `crate::watch` and are only counted. A page freed is forgotten.
#![cfg(feature = "diagnostics")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

const PAGE: usize = 4096;
/// At most this many pages guarded at once.
const MOST: usize = 1 << 20;
/// Places in the title's code kept (more are counted, not told apart).
const SITES: usize = 4096;
const STATUS_GUARD_PAGE_VIOLATION: i32 = 0x8000_0001_u32 as i32;

/// Whether landed pages are guarded now.
static GUARDING: AtomicBool = AtomicBool::new(false);
/// Whether any page is guarded (the handler's first look).
static ANY: AtomicBool = AtomicBool::new(false);
/// Pages landed on views, which are not guarded; pages not guarded for
/// another reason (not committed, not the title's, too many).
static VIEWS: AtomicU64 = AtomicU64::new(0);
static OTHER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum By {
    Not,
    TitleRead,
    TitleWrite,
    Ours,
    Runtime,
    Kernel,
}

struct Page {
    /// The copy that landed it: where its title buffer starts, and bytes.
    copy: (u64, u64),
    /// Its protection before the guard.
    protection: u32,
    since: Instant,
    by: By,
}

#[derive(Default)]
struct Tally {
    guarded: u64,
    title_read: u64,
    title_write: u64,
    ours: u64,
    runtime: u64,
    kernel: u64,
    /// Milliseconds from guard to the title's first touch, the first ones.
    after: Vec<u32>,
}

struct State {
    pages: HashMap<usize, Page>,
    copies: HashMap<(u64, u64), Tally>,
    /// Places in the title's code (instruction address) and touches; the
    /// rest counted in `more`.
    sites: Vec<(usize, u64, bool)>,
    more: u64,
}

fn state() -> MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE
        .get_or_init(|| {
            Mutex::new(State {
                pages: HashMap::new(),
                copies: HashMap::new(),
                sites: Vec::with_capacity(SITES),
                more: 0,
            })
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// What a query of the page at `at` says: committed private memory, and its
/// protection, with the bytes from `at` on that share both.
fn query(at: usize) -> Option<(u32, usize)> {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_READWRITE, VirtualQuery,
    };
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a question about an address, with room for the answer.
    let n = unsafe { VirtualQuery(at as *const _, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
    if n == 0 || info.State != MEM_COMMIT || info.Type != MEM_PRIVATE {
        return None;
    }
    // Read-write only (with whatever caching): a page read-only for another
    // reason is someone else's business.
    if info.Protect & 0xff != PAGE_READWRITE || info.Protect & PAGE_GUARD != 0 {
        return None;
    }

    let end = info.BaseAddress as usize + info.RegionSize;
    Some((info.Protect, end.saturating_sub(at)))
}

/// A landing through `copy` (null: none known) changed the pages of
/// `[from, to)` (whole pages), at the title's own address. `view`: it went
/// through a view of ours of the title's physical pages instead.
pub(crate) fn landed(copy: *const crate::shadow::Shadow, from: usize, to: usize, view: bool) {
    if !GUARDING.load(Relaxed) || from >= to {
        return;
    }
    let pages = (to - from) / PAGE;
    if view {
        VIEWS.fetch_add(pages as u64, Relaxed);
        return;
    }
    // SAFETY: the landing holds its copies until it is done.
    let Some(key) = (!copy.is_null()).then(|| unsafe { &*copy }).and_then(crate::shadow::Shadow::census_key) else {
        OTHER.fetch_add(pages as u64, Relaxed);
        return;
    };
    use windows_sys::Win32::System::Memory::{PAGE_GUARD, VirtualProtect};
    let mut s = state();
    let mut at = from;
    while at < to {
        let Some((protection, same)) = query(at) else {
            OTHER.fetch_add(1, Relaxed);
            at += PAGE;
            continue;
        };
        let end = (at + same).min(to);
        let n = (end - at) / PAGE;
        if n == 0 || s.pages.len() + n > MOST {
            OTHER.fetch_add(((to - at) / PAGE) as u64, Relaxed);
            break;
        }
        // Known before guarded: a touch at once finds them.
        let now = Instant::now();
        for page in at / PAGE..end / PAGE {
            s.pages.insert(page, Page { copy: key, protection, since: now, by: By::Not });
        }
        ANY.store(true, Relaxed);
        let mut old = 0;
        // SAFETY: committed private pages the title allocated, read-write;
        // only the guard is added.
        if unsafe { VirtualProtect(at as *const _, end - at, protection | PAGE_GUARD, &mut old) } != 0 {
            s.copies.entry(key).or_insert_with(Tally::new).guarded += n as u64;
        } else {
            for page in at / PAGE..end / PAGE {
                s.pages.remove(&page);
            }
            OTHER.fetch_add(n as u64, Relaxed);
        }
        at = end;
    }
}

/// From the exception handler: a fault at `at` (`access`: 0 read, 1 write)
/// by the instruction at `site`, `ours` if it is in one of our copies. True
/// if it was a page guarded here: the guard is gone and the instruction may
/// run again.
pub(crate) fn fault(code: i32, access: usize, at: usize, site: usize, ours: bool) -> bool {
    if code != STATUS_GUARD_PAGE_VIOLATION || !ANY.load(Relaxed) {
        return false;
    }
    // No allocation in here: the faulting thread may be anywhere.
    let mut s = state();
    let Some(page) = s.pages.get_mut(&(at / PAGE)) else {
        return false;
    };
    if page.by != By::Not {
        return true;
    }
    let runtime = !ours && RUNTIME.get().is_some_and(|r| r.contains(&site));
    let title = !ours && !runtime;
    page.by = match (ours, runtime, access) {
        (true, _, _) => By::Ours,
        (_, true, _) => By::Runtime,
        (_, _, 1) => By::TitleWrite,
        _ => By::TitleRead,
    };
    let (by, key, ms) = (page.by, page.copy, page.since.elapsed().as_millis().min(u128::from(u32::MAX)) as u32);
    let s = &mut *s;
    if let Some(t) = s.copies.get_mut(&key) {
        match by {
            By::TitleRead => t.title_read += 1,
            By::TitleWrite => t.title_write += 1,
            By::Runtime => t.runtime += 1,
            _ => t.ours += 1,
        }
        if title && t.after.len() < t.after.capacity() {
            t.after.push(ms);
        }
    }
    if title {
        let write = by == By::TitleWrite;
        if let Some(e) = s.sites.iter_mut().find(|e| e.0 == site && e.2 == write) {
            e.1 += 1;
        } else if s.sites.len() < SITES {
            s.sites.push((site, 1, write));
        } else {
            s.more += 1;
        }
    }
    true
}

/// Where our runtime's own image lies (the title's images are not the
/// system's, and our code is all in the executable): asked outside the
/// handler, read in it.
static RUNTIME: OnceLock<std::ops::Range<usize>> = OnceLock::new();

fn runtime_known() {
    RUNTIME.get_or_init(|| {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows_sys::Win32::System::ProcessStatus::{GetModuleInformation, MODULEINFO};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: questions about our own executable, with room for answers.
        unsafe {
            let module = GetModuleHandleW(std::ptr::null());
            let mut info: MODULEINFO = std::mem::zeroed();
            if module.is_null() || GetModuleInformation(GetCurrentProcess(), module, &mut info, size_of::<MODULEINFO>() as u32) == 0 {
                return 0..0;
            }
            let base = info.lpBaseOfDll as usize;
            base..base + info.SizeOfImage as usize
        }
    });
}
/// The kernel is about to write `[at, at + len)` (a file read), or the title
/// to free it (`freed`): guards there go.
pub(crate) fn lift(at: usize, len: usize, freed: bool) {
    if !ANY.load(Relaxed) || len == 0 {
        return;
    }
    let (first, last) = (at / PAGE, at.saturating_add(len).div_ceil(PAGE));
    let mut s = state();
    let s = &mut *s;
    let found: Vec<usize> = if last - first < s.pages.len() {
        (first..last).filter(|p| s.pages.contains_key(p)).collect()
    } else {
        s.pages.keys().copied().filter(|p| (first..last).contains(p)).collect()
    };
    for page in found {
        if freed {
            s.pages.remove(&page);
            continue;
        }
        let Some(p) = s.pages.get_mut(&page) else { continue };
        if p.by == By::Not {
            p.by = By::Kernel;
            unguard(page, p.protection);
            if let Some(t) = s.copies.get_mut(&p.copy) {
                t.kernel += 1;
            }
        }
    }
}

/// [`lift`] for a free of the whole allocation at `at`.
pub(crate) fn freeing(at: usize, len: usize) {
    if !ANY.load(Relaxed) {
        return;
    }
    if len != 0 {
        return lift(at, len, true);
    }
    use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
    let mut end = at;
    loop {
        // SAFETY: plain data filled in by the query.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: a question about an address, with room for the answer.
        let n = unsafe { VirtualQuery(end as *const _, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
        if n == 0 || info.AllocationBase as usize != at || info.RegionSize == 0 {
            break;
        }
        end = info.BaseAddress as usize + info.RegionSize;
    }
    lift(at, end - at, true);
}

/// The guard taken from one page, if it still has it.
fn unguard(page: usize, protection: u32) {
    use windows_sys::Win32::System::Memory::{PAGE_GUARD, VirtualProtect};
    // (A page whose guard went meanwhile, by a touch the handler has not
    // come to yet, or which is no longer what it was, is left as it is.)
    let at = page * PAGE;
    let still = query_guarded(at);
    if still == Some(protection | PAGE_GUARD) {
        let mut old = 0;
        // SAFETY: our guard on the title's page taken away again.
        unsafe { VirtualProtect(at as *const _, PAGE, protection, &mut old) };
    }
}

/// The protection of a committed private page, guard and all.
fn query_guarded(at: usize) -> Option<u32> {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, VirtualQuery};
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a question about an address, with room for the answer.
    let n = unsafe { VirtualQuery(at as *const _, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
    (n != 0 && info.State == MEM_COMMIT && info.Type == MEM_PRIVATE).then_some(info.Protect)
}

/// A frame ended (`crate::queue`'s, beside the book of faults).
pub(crate) fn frame() {
    static SETTINGS: OnceLock<Option<(Vec<u64>, u64)>> = OnceLock::new();
    static BEGAN: OnceLock<Instant> = OnceLock::new();
    /// Frames left of the window, then of its tail.
    static LEFT: AtomicU64 = AtomicU64::new(0);
    static TAIL: AtomicBool = AtomicBool::new(false);
    static DONE: AtomicU64 = AtomicU64::new(0);
    let began = *BEGAN.get_or_init(Instant::now);
    let Some((times, frames)) = SETTINGS.get_or_init(|| {
        std::env::var_os("VICTORY_READ_WATCH").filter(|v| v == "1")?;
        let times = std::env::var("VICTORY_READ_WATCH_AT").unwrap_or_else(|_| "120,180,240".into());
        let frames = std::env::var("VICTORY_READ_WATCH_FRAMES").ok().and_then(|f| f.trim().parse().ok()).unwrap_or(20u64);
        Some((times.split(',').filter_map(|t| t.trim().parse().ok()).collect(), frames.max(1)))
    }) else {
        return;
    };
    let left = LEFT.load(Relaxed);
    if left > 1 {
        LEFT.store(left - 1, Relaxed);
    } else if left == 1 {
        if TAIL.swap(false, Relaxed) {
            LEFT.store(0, Relaxed);
            let n = DONE.fetch_add(1, Relaxed);
            report(n, *frames);
        } else {
            GUARDING.store(false, Relaxed);
            TAIL.store(true, Relaxed);
            LEFT.store(*frames, Relaxed);
        }
    } else if times.get(DONE.load(Relaxed) as usize).is_some_and(|&t| began.elapsed().as_secs() >= t) {
        runtime_known();
        VIEWS.store(0, Relaxed);
        OTHER.store(0, Relaxed);
        LEFT.store(*frames, Relaxed);
        GUARDING.store(true, Relaxed);
        tracing::info!(frames, "read watch (VICTORY_READ_WATCH): landed pages guarded from now on");
    }
}

/// Every guard let go, and the log told what was touched.
fn report(n: u64, frames: u64) {
    let (copies, sites, more) = {
        let mut s = state();
        ANY.store(false, Relaxed);
        for (&page, p) in &s.pages {
            if p.by == By::Not {
                unguard(page, p.protection);
            }
        }
        s.pages.clear();
        let more = std::mem::take(&mut s.more);
        (std::mem::take(&mut s.copies), std::mem::take(&mut s.sites), more)
    };
    let mut copies: Vec<((u64, u64), Tally)> = copies.into_iter().collect();
    copies.sort_by_key(|c| std::cmp::Reverse(c.1.guarded));
    let sum = |f: fn(&Tally) -> u64| copies.iter().map(|c| f(&c.1)).sum::<u64>();
    let (guarded, read, write, ours, runtime, kernel) = (
        sum(|t| t.guarded),
        sum(|t| t.title_read),
        sum(|t| t.title_write),
        sum(|t| t.ours),
        sum(|t| t.runtime),
        sum(|t| t.kernel),
    );
    let untouched = guarded.saturating_sub(read + write + ours + runtime + kernel);
    let never: u64 =
        copies.iter().filter(|c| c.1.title_read + c.1.title_write + c.1.runtime == 0).map(|c| c.1.guarded).sum();
    tracing::info!(
        "read watch {n} ({frames} frames, {frames} more to be touched): {guarded} landed pages guarded ({:.1} MB) in {} copies; \
         first touched by the title {read} read, {write} write; by our runtime's code {runtime}; by our copies {ours}; by a \
         kernel write {kernel}; not at all {untouched}. {never} pages ({:.0} %) are in copies neither the title nor our \
         runtime's code touched. Not guarded: {} on views, {} other",
        guarded as f64 * PAGE as f64 / 1e6,
        copies.len(),
        if guarded == 0 { 0.0 } else { never as f64 * 100.0 / guarded as f64 },
        VIEWS.load(Relaxed),
        OTHER.load(Relaxed),
    );
    for ((start, len), t) in copies.iter().take(40) {
        let mut after = t.after.clone();
        after.sort_unstable();
        let median = after.get(after.len() / 2).map_or_else(|| "-".to_owned(), |m| format!("{m} ms"));
        tracing::info!(
            "read watch {n}: copy {start:#x} {:.2} MB: {} pages guarded; title read {}, wrote {}; runtime {}; ours {}; \
             kernel {}; not touched {}; median to the title's first touch {median}",
            *len as f64 / 1e6,
            t.guarded,
            t.title_read,
            t.title_write,
            t.runtime,
            t.ours,
            t.kernel,
            t.guarded.saturating_sub(t.title_read + t.title_write + t.runtime + t.ours + t.kernel),
        );
    }
    let mut sites = sites;
    sites.sort_by_key(|s| std::cmp::Reverse(s.1));
    for (site, count, write) in sites.iter().take(25) {
        let place = crate::titlewrite::place(*site).map_or_else(|| format!("{site:#x}"), |(i, o)| format!("{i}+{o:#x}"));
        tracing::info!("read watch {n}: the title's {} at {place}: {count} pages", if *write { "write" } else { "read" });
    }
    if more > 0 {
        tracing::info!("read watch {n}: {more} touches from further places");
    }
}

/// Room kept for the first touches' times of a copy (so the handler need not
/// allocate).
const AFTER: usize = 256;

impl Tally {
    fn new() -> Self {
        Self { after: Vec::with_capacity(AFTER), ..Self::default() }
    }
}

impl Default for Page {
    fn default() -> Self {
        Self { copy: (0, 0), protection: 0, since: Instant::now(), by: By::Not }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A guard set on a private page is noted when touched, and the touch
    /// goes on; one never touched is let go as it was.
    #[test]
    fn touched_and_let_go() {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree};
        crate::watch::install();
        runtime_known();
        // SAFETY: fresh memory of ours.
        let base = unsafe { VirtualAlloc(std::ptr::null(), 4 * PAGE, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) } as usize;
        assert_ne!(base, 0);
        let key = (base as u64, (4 * PAGE) as u64);
        guard_for_test(key, base, base + 2 * PAGE);
        // SAFETY: our page; the guard goes at the first touch.
        let v = unsafe { std::ptr::read_volatile(base as *const u32) };
        assert_eq!(v, 0);
        // SAFETY: as above.
        unsafe { std::ptr::write_volatile((base + PAGE) as *mut u32, 7) };
        {
            let s = state();
            let t = &s.copies[&key];
            assert_eq!(t.guarded, 2);
            // The test's own code is the executable's: our runtime's.
            assert_eq!((t.runtime, t.title_read + t.title_write, t.ours), (2, 0, 0));
        }
        guard_for_test(key, base + 2 * PAGE, base + 3 * PAGE);
        report(99, 1);
        assert_eq!(query_guarded(base + 2 * PAGE), Some(PAGE_READWRITE), "the guard is let go");
        // SAFETY: as above.
        unsafe { std::ptr::write_volatile((base + 2 * PAGE) as *mut u32, 1) };
        // SAFETY: ours.
        unsafe { VirtualFree(base as *mut _, 0, MEM_RELEASE) };
    }

    fn guard_for_test(key: (u64, u64), from: usize, to: usize) {
        use windows_sys::Win32::System::Memory::{PAGE_GUARD, VirtualProtect};
        let mut s = state();
        let (protection, _) = query(from).expect("committed read-write");
        for page in from / PAGE..to / PAGE {
            s.pages.insert(page, Page { copy: key, protection, ..Page::default() });
        }
        s.copies.entry(key).or_insert_with(Tally::new).guarded +=
            ((to - from) / PAGE) as u64;
        ANY.store(true, Relaxed);
        let mut old = 0;
        // SAFETY: our pages.
        assert_ne!(unsafe { VirtualProtect(from as *const _, to - from, protection | PAGE_GUARD, &mut old) }, 0);
    }
}
