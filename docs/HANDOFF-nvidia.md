# Handoff: the NVIDIA work (2026-11-25)

Where the NVIDIA effort stands, what was measured, what is broken, what to do next, and the
quirks that cost time. Written for whoever picks this up next, human or agent. Everything here
was measured on one PC; numbers from other machines are labelled.

**Start here (2026-11-26):** "Next, in this order" below: first the intro stutter after the first
A press, then mouths that move with the spoken lines in cutscenes.

## The machine

| | |
|---|---|
| GPU | RTX 2080 SUPER, 8 GB (Turing; waves of 32 only), driver 610.88; Windows gives the runtime a budget of about 6.9-7.2 GB |
| CPU | Ryzen 5 5600, 6 cores / 12 threads |
| RAM | 32 GB (avatarmanz's tree used 40-46 GB on their PC and recommends 64 GB: watch for paging) |
| Board | Gigabyte B450 AORUS PRO WIFI, BIOS F66d, PCIe 3.0 x16 (about 13 GB/s to the card) |
| Display | 2560x1440 at **59 Hz** (not 60: see quirks) |
| OS | Windows 11 Pro 26100; GPU timeout raised (TdrDelay 10 s, TdrDdiDelay 20 s) with `tools/runkit/Set-GpuTimeout.ps1` |
| Clocks | stock. A +100 MHz GPU overclock caused `DXGI_ERROR_DEVICE_HUNG` mid-run. |

## Branches (github.com/KingDariusStim/victory-rtx, public)

| Branch | Base | What it is |
|---|---|---|
| `rtx-next` | avatarmanz/VIctory-7900XTX `0a43566` | **The base to work on.** Their performance tree plus our NVIDIA pieces (below). |
| `nvidia` | DexNom/VIctory `2e01ab4` | The first NVIDIA tree: 16-bit stand-in copies, debug messages, audio counters, ambient light (Belobug port), run kit. Superseded. |
| `xtx-port` | `nvidia` | Five small commits picked from avatarmanz onto `nvidia`. Superseded by `rtx-next` (which has them all). |

Lineage of `rtx-next`: Marvisak/VIctory -> fxceless/victory-fork -> avatarmanz/VIctory-7900XTX
(which took work from DexNom, Belobug/romainhedouin, ZJOU, myokex, Geckomacabre and others;
their commits name each source). Our commits on top:

- `b33e1c7` copies between B5G5R5A1/B4G4R4A4 textures and their RGBA8 stand-ins converted on the GPU
  (`crates/victory-gpu/src/standin.rs`); before, they were skipped.
- `bc79099` D3D12 debug-layer messages logged as they come (`VICTORY_D3D12_DEBUG_LIVE=0` turns off).
- `a8b66ea` audio decoder (XMA/Opus) counters and failure logging.
- `5050ed1` `tools/runkit` (Run-Victory.ps1 with `-Fast`, `-Book`, `-Mode warm`; log summarizer).
- `c5bed7d` README notice (GPL modified version, credits, no game content).

Not yet run on `rtx-next`: the full `cargo test -p victory-gpu` suite (it needs the GPU while no game runs).

## Runs and what they showed

| Run | Tree | Frame rate | What was seen |
|---|---|---|---|
| 1 | `nvidia` | about 2 fps | Playable, inputs work. Pitch-black shadows, a haze. Audio: effects first, music and dialogue minutes later (slow streaming, not a decoder fault). Ended by the overclock. |
| 2 | `nvidia` + ambient | 1-2.5 fps | The ambient stand-in fires on every light resolve. |
| 3 | `xtx-port` | 0.1 fps for minutes | The picked shader fixes changed every translation, so the pipeline cache was void and all pipelines were built again in play. Copying back took 1.2-1.6 s a frame. Video memory 8.2 GB used of a 7.1 GB budget (over: Windows paged to system RAM), but **every texture was there and the cell looked right**. |
| 4 | `rtx-next`, cold cache | 2.2-2.9, later 6 fps | Lucia's face and skin missing; the picture blurring in and out. Cause: their tree keeps up to 3 GB of video memory for render targets and 1 GB for buffer copies (sized for a 24 GB card), so on 8 GB new textures stopped ("the card has no room for more; they wait", 8,700-12,400 waiting). About 1,100 indirect draws a frame dropped ("prepare failed"). |
| 5 | `rtx-next`, warm, reserves lowered | 2-7 fps | Face back. Draws dropped while pipelines are made: 5 a frame (run 4: up to 1,600); indirect drops 22 (run 4: 1,100). Card now really full: 6.6 of 6.9 GB, **31,600 textures waiting**. Issues listed below. |

Run 5 was started with `VICTORY_CARD_ROOM_FOR_TARGETS_MB=1024` and `VICTORY_CARD_ROOM_FOR_BUFFERS=0`
on top of `scripts\play.ps1`'s settings, plus `VICTORY_PIPELINE_BOOK=1`.

## Open problems (run 5), with the best explanation so far

1. **Rough, missing or partly drawn textures; transparent floors; hair that does not work.** The
   texture queue waits for room on an 8 GB card. A slot whose texture has not arrived shows the
   neutral texture: 1x1, every byte 0x80, so alpha 0.5 (`neutral()` in
   `crates/victory-gpu/src/descriptor/host.rs`). Floors and hair are alpha-tested, and 0.5 can fall
   below the cut, so they vanish. Run 3, which overran the budget instead, looked right. Not proven.
2. **Ghosting ("blur distortions").** A faint second copy of the character offset from the first;
   people outside smeared. Temporal anti-aliasing blends old frames at the wrong place: its motion
   data is missing or wrong, and at 2-7 fps every frame is a long step. Also cutscene depth of field,
   which focuses on the face: with the face missing (run 4) the focus hunted.
3. **Dark blue bleeding in the sky.** Patches with dark blue centres that spread frame to frame:
   bad values (NaN or overflow) in a sky or cloud pass carried on by temporal passes and bloom.
   Suspects: copies of the R9G9B9E5 stand-in (format 67 -> RGBA16F) are still **skipped**, so a pass
   may read a target nobody filled; or one of the six refused compute pipelines (below).
4. **Occasional black shadows.** The ambient stand-in (avatarmanz's, level 0.8) is a constant in
   place of the title's ray-traced indirect light; where that light matters most it is not enough.
   Their README says the same.
5. **Square dark blocks in hair.** Matches a bug avatarmanz lists: pixel shaders that read their
   quad's neighbours with wave reads, in which helper lanes take no part on PC. Their fix
   (`1c8dd31`, quad operations) is in this tree; check whether it covers these shaders on NVIDIA.
6. **Six compute pipelines refused** (`E_INVALIDARG`), each dropped once or so a frame:
   `02384a60fda6f2d0 2cc4c197ee29e291 2da4beb386509b77 369cbe906ea72764 5dc3b25986622949 6f225b641c5b0708`.
   They need waves of 64; Turing has 32 only. `victory-shader` splits most wave-64 programs; these
   are the rest.
7. **Stutter in the 2D intro.** The runtime makes a steady 30.0 fps there, but presents with
   `Present(0, 0)` (no vsync, `crates/victory-gpu/src/host.rs`, search `Present(`) on a 59 Hz
   display, so about once a second a frame stays up one refresh longer. Harmless; add a vsync option.
8. **Speed.** 2-7 fps. On avatarmanz's PC (RX 7900 XTX, 24 GB) the same tree does 13-20 fps indoors.
   Here the card is full and the copies between the title's memory and the card are still large
   (one frame of run 5: 66 MB of copies preparing; a dozen buffers of 10-25 MB fetched from the card
   and compared about once a frame each; in run 3 the same kind came back with 0 bytes changed).

## Next steps, in order

1. **Run 6: let textures overrun the budget, as run 3 did.** `VICTORY_TEXTURE_BUDGET_MB=5500`
   replaces the budget check with a fixed allowance (`room()` in `crates/victory-gpu/src/resource.rs`).
   If floors, hair and the cell come back, problem 1 is confirmed. avatarmanz notes that going past
   the budget ended two of their runs; watch for device loss.
2. **Residency priorities** (`ID3D12Device1::SetResidencyPriority`; neither tree calls it). Render
   targets, depth and buffers the card writes every frame: maximum. Textures drawn in the last few
   frames: normal. Textures unused for seconds, and the biggest mip levels: low. Then Windows pages
   the right things out to system RAM when the card overruns. Needs a last-used frame per host
   texture and a pass that sets priorities now and then.
3. **Lazy read-back**: built, correct as far as runs 14-16 show, off by default, and not yet a
   gain. See "The read census and the lazy read-back" below for what it found, what it does, where
   it stands and what it needs next.
4. **Retire buffers that stopped changing.** One that came back unchanged N frames running is no
   longer compared until new GPU work writes it. Smaller gain; easy.
5. **Size the reserves from the budget in code**, not by environment: `targets_room_kept()`
   (`resource.rs`, 3 GB default) and `card_room_kept()` (`crates/victory-gpu/src/shadow.rs`, 1 GB).
   For example a seventh and a twentieth of the budget, which keeps 3 GB and 1 GB on a 24 GB card.
   Worth offering to avatarmanz upstream.
6. **R9G9B9E5 stand-in copies**: converted now (`standin.rs` modes 4 and 5, format 67 <-> RGBA16F,
   with a test that runs every exponent through both ways exactly). Run 20 then showed the stand-in
   report line at "copies converted: 0 into a stand-in, 0 out of one; skipped: 0 into, 0 out of" in
   the prison yard, and the bloom unchanged: in that scene the title makes no such copies at all, so
   problem 3's cause is elsewhere. Left on its list: the six refused wave-64 compute programs
   (problem 6), the title's exposure or tonemap reading a target a dropped pass never filled, and
   resolves between stand-ins, which are still skipped. Other scenes (the open sky) may copy where
   the yard does not; the report line says.
7. **Vsync option**: `Present(1, 0)` (or 2 at 30 fps) behind a switch.
8. **Temporal ghosting**: find the pass writing motion vectors and check it runs (dropped draws,
   refused pipelines); a switch that drops TAA history would confirm.
9. **The six refused wave-64 programs** (problem 6).
10. Warm the full pipeline set once, overnight: `victory warm` without `--used` (83,561 pipelines).
11. Bigger: stream textures by tiles (partial residency; `crates/victory-gpu/src/tiles.rs` is a start).

Done and ruled out: Resizable BAR (no official support on RTX 20 cards; only a firmware mod);
a RAM disk as a cache (the copies are already RAM to RAM; the cost is waiting and comparing).

## The read census and the lazy read-back (runs 6-16, 2026-11-25 evening)

**The question.** After each submission the bytes shaders wrote go back into title memory before
the title is told its work is done (`crates/victory-gpu/src/land.rs`): 35-40 MB a frame here. On the
console the CPU reads some of that memory; on this PC the relay thread spent 30-36 ms a frame on
it, and the submitting thread 15 ms more taking snapshots. How much of it does the title's CPU ever
read?

**The census** (`VICTORY_READ_WATCH=1`, diagnostics builds, `crates/victory-gpu/src/readwatch.rs`):
in three windows of 40 frames, the 4 KiB pages a landing had just changed were made guard pages,
and the first touch of each noted. Run 6: 875, 627 and 420 MB landed; the title's own code touched
0.2 % of the pages; 98-99 % of the pages lay in copies the title never touched at all. One 94 MB card
copy (0xd6b610000) was 75 % of the bytes; a 12.6 MB and two 6 MB copies in the PC's memory most of
the rest. The relay's own breakdown: 26 ms a frame comparing PC-memory copies with their snapshots,
13 waiting for card fetches, 8 merging; the snapshots 15 ms on the submitting thread. So the
PC-memory copies cost more time than the card copy, and the lazy path was built for them first.

**What it does** (`VICTORY_LAZY_BACK=1`, `crates/victory-gpu/src/lazy.rs`; hooks in `shadow.rs`,
`watch.rs`, `host.rs`, `texture.rs`, `memory.rs`). A copy in the PC's memory of a title buffer in the
title's own private memory, 256 KiB or more (`VICTORY_LAZY_MIN_KB`), with no other copy of any of
its memory, does not compare or merge when its flight lands. Its declared ranges are noted stale,
their pages made no-access, and the snapshot kept. The first touch of a stale page faults into the
handler of `crate::watch`, which lands the piece's declared parts (copy against snapshot, as the
eager landing would) and gives the pages back. A kernel write (file reads announce themselves with
`kernel_writing`/`writing`), a free, a commit over the pages, or another copy's refresh of the same
memory lands first. `=2` lands at once but keeps the protections (verifies the machinery), `=3`
protects only and lands eagerly; `VICTORY_LAZY_TRACE=1` logs the first 2,000 events. The frame-rate
line gets a "lazy read-back per frame" line.

**What it cost to get right.** Runs 7-13 all died the same way, seconds to half a minute after the
first deferred landings: the title's own null write at image+0x23cf683 on a worker thread. Three
real bugs, in the order found:
- a whole 64 KiB piece was compared against a snapshot that only holds the declared ranges, and the
  copy's older bytes went over the title's own writes outside them (now only declared offsets are
  compared and put: `Stale.ranges`);
- `VirtualQuery` on the title's regions costs milliseconds a call (the tree knows this: `copy_mapped`);
  one per protection was 100-200 ms a frame (now asked once per copy);
- the one that crashed: when the first page of a run was already no-access (a neighbouring copy's
  stale piece on a page both share), `protect()` refused the run, put that no-access on the whole run
  and dropped the claim. A kernel write into such a page does not fault, it fails: DirectStorage's
  read failed, its retries failed, and the title worked with the bytes from before. Found with the
  trace in run 15; fixed in run 16's build, which then ran 10 minutes in the world (7-8 fps in the
  prison yard, 3,700 draws, no warnings).
Also found on the way: a landing's own writes called back into the landing on its own lock
(a thread-local flag now); landing the whole range ahead of `merge_under_writes` landed every deferred
piece within its frame (removed: the handler suffices there); 4-byte counters the title polls were
being deferred (the size floor).

**Where it stands.** Correct, and nearly no gain: in run 16's steady state 0.4 MB a frame deferred
of 35-40 MB landed, copying back still 32-37 ms. Two reasons, both structural:
1. The title's buffers overlap: a big heap has thousands of smaller buffers inside it, each with a
   copy of its own. Another copy learns of the card's writes only from the eager landing's writes
   into title memory (the kernel's dirty bits), and the host skips its refresh when nothing is
   dirty. So a copy with another copy over any of its memory is kept eager (`Shadow::alone`): 12
   landings a frame, among them the 6 MB copies.
2. The 94 MB card copy (75 % of the bytes) is not covered: its pieces need a fetch from the card at
   the touch (a fetch kit usable from any thread; the bookkeeping fits, see `relay_card_back`).

**Run 17 settled it.** Overlapped copies were let defer too, their pages noted as written in the
watch's journal (`crate::watch::landed_beside`) so the other copies would refresh through the fault
path: 637 pieces a frame deferred, 735 landed within the frame, 703 of them at faults of our own
copies, 900 ms a frame in the handler. The title's buffers lie inside bigger ones and each has a copy;
what one copy defers, the copies inside it need the same frame. The census's "touched first by our
copies: 198,179" was that, not the echo it was read as. So the lazy read-back stays as it is: correct,
off by default, on with `VICTORY_LAZY_BACK=1` (`VICTORY_LAZY_ALONE=0` is run 17's rule), worth about
one per cent of the landed bytes here.

**What would make it pay** is not in the landing but in the copies: one copy per region of title
memory, with the smaller buffers inside it as views at offsets of the same host buffer (what
avatarmanz lists first under "What is next": "view the card's own buffers where fills copy them
now"). Then the card's writes are in the one copy every view reads, no title memory stands between
the copies, and the landing is left to serve the title's CPU alone, which the census says reads
0.2 % of it. That is a refactor of `shadow.rs`'s buffer registry (`BUFFERS`, `lookup`, `card_for`)
and of every place that assumes a copy begins at its buffer's start; the lazy read-back as built is
then the right landing for it. Phase 2 (the 94 MB card copy, fetched at the touch) belongs after
that refactor for the same reason: it is the most overlapped memory of all.

Run 6 also showed ~700 draws a frame dropped "without a host pipeline" against run 5's 5: new scene
content, not a regression; `-Mode warm` after a run covers it.

## What fills the 8 GB (run 16, the prison yard, after 10 minutes)

The memory line (`memory of textures`, diagnostics builds, every 10 s): the card's budget is 7,219 MB
and **8,167 MB were in use**, 950 MB over, with 7.5 GB of the PC's memory shared with the card in use
(Windows paging). Of it: host copies the host renders to or writes, 730 alive, **3,045 MB**; host
copies of textures the title fills, 17,774 alive, 1,729 MB (36,611 let go and 1,388 made again since
the start: the sweep works); card copies of title buffers 572 MB; 1,642 texture passes made nothing
for lack of room. So the textures the title fills, which problem 1 above is about, are not the main
consumer; the host-rendered targets are, at 1440p what the console draws at. Two tools for it, both
in this tree now:

- `VICTORY_TEXTURE_MAX=<pixels>`: a 2D texture the title fills whose sides exceed it is hosted from
  the first mip level that fits (`resource.rs` `texture_max`, `capped_first`; it rides on the
  held-levels machinery, `VICTORY_HELD_LEVELS`). Belobug's tree caps streamed textures at 1024 on a
  16 GB card for the same reason. Run 18 tried 1024 here: 40 s after the first world frame the
  copies the title fills were 2,344 MB against run 16's 2,386 at the same point, the card 7,394 MB
  against 7,653. Not a lever: the title streams its own mip levels and views most textures from
  small ones already (36,000 copies "of the levels viewed alone", 922 MB on the card, against
  16.5 GB had they been whole), so what is left to cap is little. Kept as a switch.
- The memory line now lists, with every sixth one, the host-rendered copies by shape
  (`targets_by_shape`): the fourteen groups holding the most bytes, as count, width x height,
  format, flags, levels and MB. Run 19, the prison yard: 722 copies, 3,015 MB, **none idle** (the
  "sweep never lets go" line: 0 unused for 30 s). The biggest groups: 12 x 2560x1440 RGBA8 (180 MB),
  6 x 2560x1440 R16G16F, 6 x 2560x1440 RGBA16F, 5 x 2560x1440 R16G16F, 4 x 2048x2048 RGBA8 with 12
  levels, 4 x 2048x2048 R16 depth (shadow cascades), 2 x 3840x2160 RGBA8 and a 3840x2160 R32 depth,
  12 x 1280x720 R16G16F, then a long tail of ~1.9 GB in smaller ones. That is a renderer's working
  set at this resolution, not duplicates: the console's 10 GB of fast memory affords it, 8 GB does
  not, together with 1.8 GB of the title's textures and 0.6 GB of buffer copies.

So the lever for this card is not in the copies but in what the title renders: the **Series S
profile** (`victory run --console series-s`; the run kit now takes `-Console series-s`), under
which the title sizes its targets and textures for a 1440p-class, 8 GB-class console, or an
internal-resolution cap if the profile alone does not fit. avatarmanz's findings list the S profile
under "measured and not worth it" for speed on a 24 GB card; for memory on an 8 GB card it is the
first thing to measure. Residency priorities would not help here: with every target in use each
frame, paging them is a stall a frame.

**Run 21, Series S** (`-Console series-s`), 50 s after the first world frame: host-rendered copies
528 alive **1,249 MB** (X profile at the same point: 2,585 MB; settled, 3,015), the card **6.5 GB
of 7.2** with the texture sweep only just started (X: 7.4-7.9 GB at that point, 8.2 settled). The
biggest target groups are now 1080p-class (2 x 1920x1080 RGBA16F) with 1536x1536 shadow cascades
in place of 2048; the 3840x2160 buffers and the 2048x2048 mip chains stay. First world frames
5.5 fps against 2.2-3.2 on X. The run was cut short at 199 s by a stale driver script of the
previous run, not by a fault. **Run 22**, the full one (8 minutes, after a `-Mode warm`): settled
at 7,050-7,090 MB in use of 7,219, **under the budget and steady**, where the X profile sat 950 MB
over; "passes that made none for lack of room" stayed at 8 all run against 1,642; targets 713 at
1,370 MB; the texture copies grew to 3,389 MB because the sweep keeps textures longer while there
is room (`vram_keep`), which is the intended trade. 7-8 fps in the prison yard. The picture is the
same as on X otherwise: the orange wash and the blur are not memory. For an 8 GB card the S profile
is the setting to play with; the run kit's README says so.

## The per-draw view slots (the finding behind the dropped draws)

Every frame line of runs 5-22 drops 450-630 draws "without a host pipeline (0 being made)" and
1,300-1,850 indirect ones. The warm changes nothing (run 22 was after one), and the detail lines
name the reasons: 266 a frame are stages of the ray-tracing structure builder, off by design; 1 a
frame is the refused wave-64 compute `369cbe906ea72764`; 1 a compute that does not translate
(`8b437ab61d8008fe`); and **256 a frame are indirect draws whose "prepare failed"**, with the
comment in `draw.rs` saying what 256 indirect rectangle draws a frame are: the title's tiled
lighting. The recording line says why they fail: **"dynamic slots: 12,310 tables asked, 2,083
refused, 149 ms searching"** a frame in the world. On NVIDIA a view heap holds 1,000,000 views
(`PORTABLE_VIEWS`): the title's slots take 737,856 and the per-draw ("dynamic") area is 2^18 =
262,144 (`descriptor/host.rs`, `DYNAMIC`; AMD gets four times that). A draw's table gets a
contiguous run as long as the shader's highest table offset plus one, holes included; a run is
given back when the list's submission has run (`tables_done`); a table that finds no run is
refused and its draw dropped, after a search of the whole ring when enough slots are free but in
pieces. So the lighting draws and some 2,000 other tables a frame go unmade. This is the likeliest
single cause of the dark and wrong lighting here, and it costs 149 ms a frame of searching on top.
Run 23, with the reason named on each "prepare failed": all of them **"binding failed"**, and most
are **indirect mesh dispatches (kind 10)** of dozens of different mesh pipelines at 10 to 63 a
frame each (`[mesh 7972997d15fd9e69, pixel 9f14b72117e2abec]` 63 a frame, `[mesh
6016f52cd61f30f6]` 60, and so on), the tiled lighting's 256 among the rest. Mesh pipelines carry
the longest tables, so they are the first refused a run. That is world geometry going undrawn
every frame on this card, which the X and S profiles share alike.

Two fixes, both more than an evening:
1. **Compact tables**: have the translator lay out each pipeline's descriptor tables with only the
   offsets its shaders read (renumbered 0..k, in the root signature's ranges and the shaders'
   accesses alike), so a table whose shader reads three of 41 entries takes three slots. The host's
   table code (`draw.rs` `fresh_tables`, `n` from the highest used offset) then shrinks by itself.
   Changes the shader cache key.
2. **A better allocator** for the dynamic area: size classes or a buddy scheme in place of the
   first-fit ring (`HostHeap::dynamic`, `release`, `too_long`), so free slots in pieces still serve
   small tables and the searches go. Add first a histogram of table lengths and the free count at
   each refusal to size the classes.
Cheaper and worth trying first: `VICTORY_DYNAMIC_SLOTS` is only read on uncapped hosts; on this
one the split between the title's 737,856 slots and the per-draw 262,144 could be moved if the
title's slot indices above some bound are rare (the frame line's "texture slots written so far"
says how many slots the title writes; the handoff's run 4 note says it used 737,853 of 737,855,
so probably not). Runs 7-16 were driven unattended (a
PowerShell driver pressed Enter with a held key, took screenshots and logged the frame rate; a
`SendKeys` press is too short for the title to see).

**Fix 2 is in (2026-11-26, `descriptor/slab.rs`).** The dynamic area is cut into pages of 256
slots. A page is free, or carved for one size class (every length to 16, then 24, 32, 48, 64, 96,
128, 192, 256), or part of a run of whole pages for a table longer than 256. A table takes a run
of the smallest class that holds it, popped from that class's free list, carving a page from the
pool when the list is empty; giving back pushes it again, and a page whose runs are all back
returns to the pool for any class (free runs noted before that are skipped by a generation
count). Take and give are a pop and a push under the lock, with no search, so the 149 ms goes;
and free slots in pieces serve every table of their class, which the ring could not. The cost is
the round-up inside a class (a 17-slot table takes 24) and pages held by a class while partly
used. Each page keeps a bit per run, so a run given back twice is not handed out twice. The
recording line's "dynamic slots" counts stay; a new line after it, "dynamic slots now", gives
the free slots, the pool's pages and the free runs per class, to see whether a class starves.
`VICTORY_DYNAMIC_SLOTS` and the area's size are as before. Fix 1 (compact tables) is still worth
doing: it shrinks what every table asks for.

**Runs 24-27 (2026-11-26 01:00-01:40): the area is full, not in pieces.** The slab's first form
(free runs last given, first taken) refused 2,700-3,600 of 10,600-11,700 tables a frame, more than
the ring's 2,083, with 78,000 slots free and 4 pages in the pool: every class spread over many
pages, each held by a table or two. The second form (lowest page first, borrowing a larger class)
refused about as many, and run 25 ended with 2,924 slots free and no page in the pool: the 262,144
slots are simply taken. The search time fell from 149 ms to 6-11 ms a frame either way. Run 27
counted the demand (new tables a frame, in the prison cell): **7,178 made fresh for a constant
buffer view, 198,364 slots**; 392 past the title slots the host mirrors (1,474 slots: NVIDIA's cap
is not the cause); and **830 tables over 64 slots long hold 145,612 slots, 73%** (175 each). A
table costs its highest offset plus one, holes included, so the long ones are mostly holes. The
fix is less demand, not a better allocator: share a table made by several lists in a frame, or
lay out tables with constant buffer views compactly (they are always made fresh, so their layout
is ours; the translated shaders read descriptors as loads at fixed offsets from the table's
index, `lower.rs` ~3022, so the translator would renumber those offsets).

**Runs 28-32 (02:00-02:40): what serves the slots now, and what did not work.**
- Run 28 logged which offsets the long tables' readers name: 60 kinds read 1,187 of their 9,182
  slots (13%; a table of 101 slots read at its 101st alone). Tables made by another list in the
  same frame are few (sharing them would save little).
- Run 29 tried **sparse tables** (`descriptor/slab.rs` `Sparse`, `VICTORY_SPARSE_TABLES=1`, now off
  by default): a long table takes slots at the offsets it reads alone, its holes on other tables'.
  The world came out corrupt everywhere (striped and noisy surfaces): **some shaders read table
  slots past the entries their programs list**, which then held other tables' views. Compact
  tables in the translator would break the same way. Two thirds of the long tables found no place
  anyway (90-135 ms of searching).
- The allocator, compared in the same stretch of play (refused share of tables a frame):
  the old ring 16.8% (run 23), size classes 27.8% (run 25: rounding and pages held by a class),
  best fit 23.0% (run 30: 51,810 slots free in 7,718 crumbs, the longest 102), **next fit over
  free stretches 16.5-16.7%** (runs 31-32), the ring's share without its 149 ms: a table that no
  stretch holds is refused at once (an index by length), and the walk from the cursor stops after
  64 stretches for the shortest that holds it. That is `descriptor/slab.rs` `Extents`, in use.
  Tables come and go in about the order of a frame, so placing them in that order keeps what is
  given back in long stretches; nothing placed better.
- So the allocator is not the lever: a frame's tables need more than the 262,144 slots. The next
  step (in progress): constant buffer views out of the tables. A table is made fresh only because
  it holds one; if the translator reads a table's constant buffer views at heap indices the host
  gives it (a host word per view, `USER_CBV_INDEX`), the rest of the table stays on the title's
  own slots, holes and all, and a long table costs one or two slots instead of 175.

**Done (run 33, commit b901e86): constant buffer views out of the tables.** The translator
(`lower.rs` `cbv_index`) reads a table's constant buffer view at a fixed offset at a heap index
the host gives (`USER_CBV_INDEX`), and asks `USER_TABLE_WHOLE` for a table one of whose views it
reads any other way (`note_whole_table`). The host (`draw.rs` `tables`, `cbv_slot`) gives each such
view a slot of its own with what it read at the draw, and leaves the rest of the table on the
title's slots (`VICTORY_CBV_SPLIT=0`: whole tables as before). Run 33: **tables refused 0.02% a
frame** (16.7% before), indirect draws dropped 0 by the end of the run (about 1,900), no long
table made whole, 34,670 slots free in 651 stretches; the cell clean in the captures. A
translator change re-translates every shader: `-Mode shaders` (194 s with 8 processes) and
`-Mode warm` (2,560 pipelines, 316 s); the draws no longer dropped then need pipelines no book had
(run 33 ran at 2.5-3 fps while 600 were made in play; a second warm after it).
**The blur was the refused tables.** The opening holds a long shot of Lucia at the cell window:
in run 31 (before) it is the banded blur of "0/20 vision", thickening and never recovering; in
run 34 (after, 4,000 frames of it) it is sharp and clean throughout, Lucia's hair no longer red.
Run 34: refused 0.05%, 5.5 fps with 4,405 draws a frame (run 32: 6.8 fps, 3,788 draws, the rest
dropped).

**Upstream merged (runs 35-37, commits c4e177f and 0ef7677).** avatarmanz's 33 commits (frame
meter thread, heap records read in place, up queue, textures shown in their slots only once the
lists sent before them ran) merge with one conflict in the meter's lines. Their pooled textures
at 64 KiB multiples took this card over its budget (7,623 of 7,219 MB) and the frame's card time
from 118 to 430 ms (2.0 fps); `VICTORY_POOL_SMALL` unset is now on for cards under 12 GB, and the
merged tree runs as before (run 37: 5.58 fps, 6,993 MB, 0.02% refused).

**The black screens before the world (run 37-39, commit a440f72).** The title presents two planes
on every frame (descriptions 72 bytes apart): the game's picture, and over it its 2D (menus,
title screen, loading cards, HUD). Only the first was shown. `host/overlay.rs` blends the second
over the first (premultiplied alpha, scaled to the first's size) into a texture of the window's
and copies that to the swap chain (`VICTORY_OVERLAY_PLANE=0`: the first alone). Run 39: the
title screen, the logos, the illustrated cards and the loading screens show; the world as before.
Not read yet: the planes' source and placement rectangles, colour spaces and filters.

**What the refusals look like on screen (run 26, `VICTORY_CAPTURE_WORLD`).** The presented frames
every 8th frame show, walking from the cell to the yard: a rectangle of an older picture over the
new one (frames 5075-5123 of that run), the "jumps to frames a second or two old" seen in play;
and from 5315 on a blur with hard bands that thickens frame by frame and does not recover, the
"0/20 vision". The first is what a tiled pass leaves when some of its tiles' draws are refused;
the second a temporal pass feeding on its own output after a bad input. The cell's frames before
it (1200 frames into the world) are sharp.

## Next, in this order (from the user, 2026-11-26, after run 40)

Run 40 (`victory-data/logs/run-20261126-110925.log`, the tree at 9cb2adc, Series S profile, the
pipeline book warm) is the baseline for both targets.

### Target 1: the stutter in the intro animation, about 10 s after A is first pressed

**Fixed in the tree after 72a6d2f (runs 41-50, 2026-11-26); the user has yet to confirm by eye.**
What it was, and how it was found:

- New per-frame log (`victory_gpu::frame`): `frame intervals: longest N ms, K over 40 ms` every
  second, `frame hitch N ms` for any present interval over 50 ms, and `button A pressed` /
  `button Menu pressed` (`victory_hle::input`) from the pad poll, so a hitch can be lined up with
  the press. `VICTORY_KEYS=14:Enter` presses A at a fixed time for a run that repeats.
- A screen sampler beside it (a 256x256 patch of the window hashed as fast as the desktop gives
  it, ~60 a second) counts how long each distinct image stays up: that is what the eye sees,
  and it agrees with the frame log.
- **It was not pipelines.** Every run after `-Mode warm` makes none (`host pipelines all made
  made=0 loaded=…`): the `host pipeline shaders=` lines are library loads, and the frame line's
  "waiting for pipelines" is 0.0 ms through the intro. The counts in run 40 were a red herring.
- **It was thread starvation.** The timeline (`VICTORY_TIMELINE=14.5,60,<file>`) showed the
  long frames as "woken, not running yet": the submitting thread had asked `victory-heap-sync`
  for its round and notified it, and that thread got no processor for 80 ms to 1.3 s. At that
  moment the title's job threads burst over all 12 processors: with `-Fast`
  (`VICTORY_TITLE_PRIORITIES=0`) they run as the title set them, 6 at "time critical" and 13 at
  "highest", while our frame-critical threads ran at "normal".
- The fix (`cores.rs`, `raise`): the threads a frame waits for (`victory-submit`, `-relay`,
  `-late-wait`, `-heap-sync`: those that call `keep_on_fast_cores`) run at "time critical"
  (`VICTORY_LANE_PRIORITY=2`, the default; `1`: "highest", which still lost 75 ms behind the
  title's time-critical threads; `0`: as before). They run for microseconds between waits, so
  the title loses nothing measurable.
- Before: longest present interval in the 15 s after the press 0.4-1.3 s, 7-16 screen holds
  over 50 ms per 25 s. After: longest 59 ms once, else under 50 ms; the only hitch left in every
  run is 68 ms at +15.8 s, and the timeline shows it is the title's own `Sleep` of 56 ms on its
  render thread (content-driven: the same instant in every run).
- Also added: `VICTORY_VSYNC=1` → `Present(1, 0)` (not needed for this; left for the once-a-second
  refresh hitch of open problem 7), and a `slow asking of the write watch` line with a breakdown
  (journal, hold, kernel, guards) for any `watch::writes` over 100 ms.

The original analysis, kept for the record. What the user sees: right after the first A press
the intro animation stutters for about ten seconds, then runs smoothly. What run 40 showed:

- The 5-second frame meter (`victory_gpu::host: N fps`) reads a flat 30.0 fps from 15:09:41 to
  15:10:11, so the meter's averages hide it. **First step: a per-frame measure.** Log each
  present's interval and, every second, the longest interval and how many intervals were over
  40 ms. Then find the A press in time: log it in `victory_hle::modules::gameinput` (a button
  going down), because nothing marks it now.
- **The main suspect is pipeline creation in play.** The book is warm, yet run 40 made 40-126 host
  pipelines a second from 15:09:31 to 15:09:49, and again 30-83 a second from 15:09:55 to
  15:10:00.
  - Count the `host pipeline shaders=` lines per second (an awk over the timestamps does it).
  - `pipeline::make` waits up to 500 ms a frame for a pipeline (`host pipeline wait budget per
    frame millis=500`). A pipeline that is new to the driver, or loaded from the library on the
    submitting thread, is a hitch.
  - Check whether these pipelines are library loads (`pipeline/host.rs`, `library::pipeline`:
    `LoadGraphicsPipeline` before `Create…`) or real creations. If they are real, the book misses
    them: compare their names with the book, and check whether `-Mode warm` builds the 2D intro's
    pipelines at all.
  - Fixes to try, in order:
    - build them ahead from the book at start;
    - make them on worker threads and skip the draw until they are ready (no wait), as the run's
      `VICTORY_SKIP_PIPELINES` switch hints;
    - lower the wait budget for 2D-only frames.
- **Shader translations in play**: 50-120 `shader translated` lines a second, even where no
  pipeline is made (15:10:01-15:10:16). `Shader::translated()` (`pipeline.rs`) runs on first use,
  and a translation that misses the disk cache costs milliseconds on the thread that needs it.
  Count hits against misses, and translate the book's shaders ahead.
- Other suspects, for after that:
  - textures uploaded in the intro (12.7 MB a frame from 15:09:46 to 15:09:56: probably the intro
    movie's frames);
  - the no-vsync present: `Present(0, 0)` on a 59 Hz display, `host.rs` around line 9208, open
    problem 7. That one makes a regular one-refresh hitch about once a second, not a ten-second
    burst. Add the vsync switch anyway: `VICTORY_VSYNC=1` → `Present(1, 0)`.
- Done when: the per-frame log shows no interval over 50 ms in the 15 s after the A press, and
  the user confirms by eye.

### Target 2: mouths that move with the spoken lines in cutscenes

**Looked at on 2026-11-26 (runs 51-62), not fixed. What is known now:**

- **Ruled out: the refused and untranslated compute programs.** They are named (`victory translate
  shaders.rpf --list` with `VICTORY_TRANSLATE_ONLY=<hashes>` prints the pack's record names):
  `alphaResolve:CS_AlphTransmissionPrefilterMip` (2da4beb386509b77),
  `GpuParticleUpdate:CS_Visibility` (369cbe906ea72764), `sr_tracing:CS_Tiling_Generate_Opaque`
  (2cc4c197ee29e291), `vfx_triangleClip:CS_DefragHeap_ComputeRelocations` (02384a60fda6f2d0),
  `vfx_triangleClip:CS_CollectTriangles_Meshlets` (6b82bc7d22016cb5), `water_shading:
  CS_TransmissionVolumetrics_Tiled[_LocalLights]` (5dc3b25986622949, 6f225b641c5b0708). Three more
  (477b52e73ddeb6f2, 17af85f170e66f30, 8b437ab61d8008fe) fail to translate for wave 32 ("a lane mask
  exchanged where not every lane runs"); they write BC-format textures and are not in `shaders.rpf`,
  the executable, `common.rpf`, `data.rpf` or `textures.rpf` (their containers are in the scratch
  dumps a run makes with `VICTORY_SHADER_SAVE`). None is facial.
- **Ruled out: the audio clock.** Speech is Opus through `XApu` (6,000+ stream activations a run,
  dozens live at once: ambience uses it too), not XMA. The DSP (`XDsp`) only ever gets convolution
  streams (reverb). Reporting the decoder's progress in the result's `state`/`state_data`/`usec`
  (`VICTORY_APU_PROGRESS=1`, off; they were always 0) changed nothing. Raising the title's idle
  threads (`VICTORY_TITLE_PRIORITIES=1`) changed nothing.
- **Faces do animate** (head turns, eyes, brows): the facial system runs. Mouths stay shut while
  speech-level, syllable-modulated audio plays (a WASAPI loopback loudness log beside screenshots
  4 times a second; tools in the session's scratch folder: `loud.ps1`, `shots2.ps1`, `sheet.ps1`).
- **Where the mouth shapes are made.** The pack has a GPU facial pipeline: `ped_standard_head_cs:
  CS_ComputeBlendshapeMappings` (61e54e90adccaf6a), `CS_BlendshapeTranslationsHSSGraphOutputExtract`
  (970c4fbb3b017c0e), `CS_ReconstructBlendshapeNormals` (e4a7e088c7780ce8), `CS_ComputeStickyLips`
  (068c90badc86f1fd), `CS_AccumStretch*`, and a rig solver run as a compute graph,
  `compute_graph_ops:CS_*` (65 programs: Add, BiasedDot, Clamp, Concat, ...; 24 used in the
  cutscene). In the close-up frames (`VICTORY_CAPTURE_FRAMES=1500,1510,...`) they are all
  dispatched, none dropped, and their null table entries (`970c…` `Srv s0+0`, `Uav s0+40`) are
  the title's own null descriptors (resource 0, 0 elements), not ours. All translate exactly
  (no approximate wave-64 forms; `068c…` with its early-outs left out).
- **So the remaining suspects are the results**, not the dispatches: either the rig solver's
  input (the animation's facial pose/viseme weights from the CPU) is at rest, or one of the graph
  ops computes wrongly on this translation (reductions such as `BiasedDot` are the ones to doubt).
  Next step: read back `970c…`'s output buffer (`Uav s0+48`, 1.4 MB) or the graph's output across
  two frames of a line and see whether it changes; if it never does, test the graph ops one by one
  on WARP against a straightforward host build of the same op.

The original plan, kept for the record. What the user sees: in cutscenes the dialogue plays, but
the characters' mouths do not move. Candidates as first listed, from most to least likely:

1. **Facial animation computed on the card and dropped.** Seven compute pipelines are refused in
   run 40: the six listed in open problem 6 (they need waves of 64; Turing has only 32), and a new
   one, `6b82bc7d22016cb5`. Each is dropped once or so a frame. If one of them is the face's blend
   shapes, its skinning or its bone palette, the face holds its rest pose.
   - Find which passes they belong to: capture a cutscene frame with
     `VICTORY_CAPTURE_WORLD=1` and the shaders with `VICTORY_SHADER_SAVE`, then read what each
     one writes.
   - Then split them for wave 32 in `victory-shader`, as the translator already does for most
     wave-64 programs.
   - A quick test first: do the mouths move on avatarmanz's 7900 XTX, which runs wave 64
     natively? If they do, this is the cause.
2. **Lip sync driven by the audio's clock.** These titles usually drive visemes from the dialogue
   stream's playback position, or from lip-sync data loaded with the stream.
   - Dialogue arrives minutes late on a first run (streaming; see "Quirks"). A line that starts
     before its lip-sync data is in may play with a still face.
   - The position the HLE reports for a voice may not advance: `victory_hle::modules::apu`, `dsp`
     and `audio` (the coprocessor commands, and the position or "samples played" a stream reports
     back).
   - Test: log the reported position of the dialogue stream through one spoken line, and check
     whether it moves smoothly and ends at the stream's length.
3. **Dropped draws of the face itself**: a separate face mesh or a decal pass. Run 4 had the face
   missing outright. Since the view-slot fix, 0.02% of tables are still refused; check that none
   of them are a face draw in a cutscene, using the capture listing.

### Target 3: occasional triangle glitches (added by the user on 2026-11-26)

Not seen yet by this session: 143 screenshots a second apart in the prison cell after the first
cutscene showed none, but the scripted keys (`VICTORY_KEYS=140-170:W,...`) hardly moved the
character there, so that scan covers one static view. Ask the user where and when they appear.

The lead: two of the refused wave-64 programs are the VFX triangle heap,
`vfx_triangleClip:CS_CollectTriangles_Meshlets` (6b82bc7d22016cb5) and
`CS_DefragHeap_ComputeRelocations` (02384a60fda6f2d0). The rest of that family runs every frame
(`CS_DefragHeap_ProcessFreeRequests`, `_PostAllocateValidation`, `CS_RelocateVfxStartIndexLocation`
as an indirect dispatch, `CS_ClearDecal`): a heap that is defragmented without its relocations
computed, and never fed new triangles, is a plausible source of stray triangles when a decal or
VFX is spawned. Both need "LDS in a group over 32, an LDS read and write in one lane-branch
region" (`VICTORY_WAVE_WHY=1`), and every retry form keeps 64 lanes.

Upstream `avatarmanz/VIctory-7900XTX` main (c9c932c, 133 commits past our merge base, checked out
in `victory_repo/victory-upstream`, built) translates one of the seven refused programs so that
this card accepts the pipeline, `sr_tracing:CS_Tiling_Generate_Opaque` (2cc4c197ee29e291); the
other six, these two among them, are still refused there (`translate --pipelines`). The three
untranslatable ones fail there too. So the fix is translator work of our own, or, cheaper,
skipping the whole triangle-heap family together so its consumers see an empty heap rather than a
half-maintained one, if the glitches turn out to be theirs.

How to get there quickly: the first cutscene with dialogue (the prison yard into the diner
flashback). Use `Drive-Run.ps1 -EnterAfter` to skip the menus, and capture a few hundred frames
in which a line is spoken. Done when: the mouths move in time with the lines in that cutscene.

## The frame rate (2026-11-26 evening; the user's bar: 15 fps in gameplay and correct shading)

The user watched run 151644: the intro still stutters to the eye although the presents are clean
(one 76 ms frame in the 15 s after the press), and the world ran at 4-5 fps. They then asked for
the frame rate first: "characterize what's eating at the framerate, find the slowest functions and
try to accelerate them", later "keep working until you get at least 15 fps during gameplay and
correct shading". Their hypothesis for the intro, to check last: stale draws showing up a couple
of milliseconds late (frames repeated or out of order, presents on time).

**Where the time went (the diner cutscene, the heaviest scene so far):**

- Timeline (`VICTORY_TIMELINE`, `VICTORY_CARD_SPANS=1`): the card ran 146-178 ms a frame over
  11-13 submissions and idled 50 ms between them; our CPU threads were nearly idle. GPU-bound.
  `nvidia-smi`: 100% utilisation at only 100 W of 250, 4% memory utilisation, PCIe 3.0 x16 at
  0.1-1.3 GB/s: the SMs were stalling, not computing.
- Per-command card times (`VICTORY_GPU_TIMING=1 VICTORY_D3D12_DRED=1 VICTORY_LATE_WAIT=0`: every
  list waited for and every noted command stamped; `copystat`'s "the card's time by command"
  every 5 s): 151 ms a frame of commands, 95 ms in 3,200 dispatches, 48 ms in 4,900 indirect
  dispatches, 5 ms in all the draws. The top: `bvh_skinning:CSComputeSkinningRigidVelocity`
  (8f9eb68d0c6dd5cf) 50-70 ms a frame in dispatches of 900-2,768 groups taking 4-13 ms each,
  `indirect_light_probes:CS_Primary_GatherCopy` 12-18 ms in one dispatch, an INT8 dot-product
  network not in shaders.rpf (8e614708143ef917, 1,664 `dot4AddPacked`: the title's FSR4 upscaler,
  "FFX FSR4 ... falling back to NewBE") 12 ms, `indirect_diffuse_lighting:CS_ScreenProbes_TraceRays`
  6-12 ms. `dxc -dumpbin` of the skinning kernel (scratchpad `cdxil/*.dxil`): a per-thread linear
  search over a table of 176-byte records, two dependent loads an iteration, up to 4,096
  iterations; the table is a 998 KB title buffer.
- **The cause: title buffer copies in the PC's memory.** `shadow.rs` `default_card_limits_mb` gave
  cards of 12 GiB and under (16 MiB minimum, 512 MiB) for copies in the card's memory, and
  `settings.local.ps1` had `VICTORY_CARD_ROOM_FOR_BUFFERS=0`, so nearly every title buffer lived in
  `shared_buffer` memory (CUSTOM heap, pool L0: system memory over PCIe) and every dependent load
  of those kernels crossed the bus. Windows of title memory (`VICTORY_CARD_WINDOWS_MB=2048`) alone
  gave a tenth; buffers of 16 MiB and up (`VICTORY_CARD_ROOM_FOR_BUFFERS=1`) 5%; **every buffer of
  64 KiB and up (`VICTORY_CARD_BUFFERS_MIN_KB=64 VICTORY_CARD_BUFFERS_MB=1536`) took the card's
  command time from 150-230 to 59-90 ms a frame**, the rigid skinning from 50 ms to 0.8, and the
  cutscene from 4-5 to 8-11 fps (runs 155020-155513). Now the default for such budgets (commit
  after 86c8bbc); `settings.local.ps1` adds `VICTORY_CARD_WINDOWS_MB=1024` and
  `VICTORY_TEXTURE_BUDGET_MB=1536` so the copies find room (the card then sits at 6.4-6.7 of 7.1
  GB). The texture cap costs textures ("walls losing textures" may be partly this); not tuned yet.
- Ruled out: the 700 full UAV barriers a frame (`VICTORY_BARRIER_TEST=both`: no change), telling
  the title its GPU frame takes 40 ms (`VICTORY_GPU_LOAD_MS=40`: no dynamic resolution response),
  reporting a 1080p display (`VICTORY_DISPLAY_MODE=1920x1080`: the title keeps a 3840x2160
  viewport and 4K targets anyway).
- After the fix, steady state in the cutscene: 79 ms a frame of commands over 8,000 commands.
  By family (top 40 kinds of each report, 56 of the 79 ms): the FSR4 network 12 ms,
  `ped_skinning_cs` 9.6 ms in 215 dispatches of 25-40 us, strand hair (`smh_*`) about 13 ms,
  `compute_graph_ops` (the facial rig) 5 ms, `scattering_transmittance_system` 3 ms, our alias
  copies 2.3 ms in 78. Plus 25-30 ms a frame of card idle between submissions (many small
  causes in the chain; the submit thread spends the frame waiting for the card). The window
  shot at the end of the cutscene, Lucia's hair filling the screen, holds 8.8 fps.
- CPU profile (`VICTORY_PROFILE=100,140,2,<file>`, fixed this session: the sampler died in
  `crash::unwind_into` on program counters outside every image; symbolise with
  scratchpad `symb/` (the `pdb` crate) against `target/diag/victory.pdb`): over 40 s the title's
  two busiest threads used 24 and 18 s, `victory-submit` 15 s (of it `shadow::wide_each
  refresh_fronts`/`Batch::update`, the copies' refresh, about a third), eight list threads 6-8 s
  each (`draw::State::bind`/`tables`), the relay 5.6 s.

**Tried after that, 2026-11-26 evening (all with the buffer copies on the card):**

- `VICTORY_REFUSE_PIPELINES=8e614708143ef917` (the FSR4 network, 12 ms): the title asserts on the
  pipeline result (`fsr4_common.cpp:1721`) and dies. No fallback that way.
- **The card's idle gaps are one wait, found in the log's "waited for flights, per frame"
  line.** In the window shot (Lucia's hair filling the frame, the "Move around" prompt: the first
  gameplay) the submit thread stands **60-80 ms a frame** for one fill of 0x180-0x230 bytes of
  upload memory out of the title's constant ring, which lies in the 174 MB heap the skinning
  passes declare written as a whole; the title copies GPU-computed bytes into the ring with a
  buffer copy at its place in the list just before the draw that reads them. `card_steps`
  (`VICTORY_CARD_STEPS=2`, in `-Fast`) would do the fill on the card but refuses the submission:
  "a copy at its place in a list writes what it reads" (the copy and the fill are in one list, so
  `VICTORY_PLACED_ORDER=1` cannot cut between them); with `VICTORY_COPIES_IN_PLACE=0` "a copy done
  on the card is read through another host copy"; with `VICTORY_FILL_TITLE=1` the fills go on but
  the submission then waits "for what a copy at its place writes" (the fill does read the copy's
  result). A new switch `VICTORY_PLACED_CUT=1` (cut the host list behind a placed copy, so the
  order path can split the submission) **hangs the title at the loading screen** (run 165640, 11
  draws a frame for ever; cause not found, left off). The fix that remains is upstream's renderer
  port 5 (`VICTORY_PORT_CBV=1`, 3920dc2, not transplanted): constant buffers read as raw bytes
  from the title's memory where it lies on the card, so there is no CPU fill to wait for. Expect
  the window shot to go from 8.8 to about 13 fps from this alone.
- `VICTORY_WIDE_THREADS=<n>` added (the copies' refresh helpers; 3 on this 12-thread CPU); not
  measured yet.
- **`VICTORY_BVH_BUILDER=1` is what the shading needs.** The title's indirect light, bounce and
  lamp/sun shadows are ray traced against structures a GPU builder maintains; this fork keeps the
  builder off (docs/fork-ray-tracing.md) and `VICTORY_AMBIENT=0.8` stands in. Off, rays never hit
  Lucia: in the window shot her hair is a glowing blonde/orange mass. **On, it is dark brown with
  a rim light and the wall beside it is lit right** (run 170213, scratchpad `bvh1`). The price
  here: 5.3-6.4 fps (110-130 ms of GPU a frame, and the fill wait above grows to 113 ms because
  the fills read buffers the builder stores into), interiors without the ambient stand-in are
  dark, and the builder's known gaps (overlapping dynamic structures, a dark polygon over Lucia
  in one shot). With the translator's 20 upstream commits all its stages ran on this card.
- The three shading complaints, then: hair colour and the ray tracing through the hair are the
  builder being off (plus whatever its gaps add); the walls losing textures is still open, with
  the dropped unordered writes to block-compressed title textures as the lead (below).

**What I would do next, in order:** (1) transplant renderer port 5 (`VICTORY_PORT_CBV`) and
measure the window shot; (2) a per-command timing run with the builder on (started as run
171x) to see what its stages cost here, then look for the same "system memory" class of loss in
`memory.rs` (the builder's title memory by address: 1,073 MB in 20 arenas on the card, 122 pages
a frame "had no copy"); (3) the BC unordered views; (4) group doubling in the translator.

**Shading leads from the logs (the user: Lucia's hair reads blonde/red in the window shot, the
ray tracing through her hair looks wrong, walls lose textures):**

- `sr_tracing:CS_Tiling_Generate_Opaque` (2cc4c197ee29e291), a tiling pass of the software ray
  tracer, was refused on this card and dropped every frame. Upstream's translator runs it as
  local 32-lane waves; 20 of upstream's shader-crate commits are now transplanted (cherry-picked
  from xtx/main 2de9980 onto rtx-next; the translate-tool one, 9b7dc77, skipped) and the pipeline
  is created. The other six refused programs emit and validate but the driver still says "The
  parameter is incorrect" (they keep 64 lanes).
- "placed unordered texture view not mirrored (null: writes through it are lost)": 4,096+ times a
  run, formats 71/72/98 (BC1/BC7). Compute shaders write block-compressed title textures through
  unordered views, as the console allows and D3D12 does not; the writes are dropped
  (`descriptor/host.rs` `make`, `View::Placed(true, ..)` of a texture). Likely the runtime texture
  compression whose output the walls wait for. Fix: a UINT alias of the BC texture (placed on the
  same heap, or castable formats) for the view, and the host copy as the source of truth after a
  shader wrote it.
- The three computes that do not translate ("a lane mask exchanged where not every lane runs":
  477b52e73ddeb6f2, 17af85f170e66f30, 8b437ab61d8008fe) write BC textures too, 1.3 dispatches a
  frame each, dropped.

## How to run

```powershell
# once: copy tools\runkit\settings.example.ps1 to settings.local.ps1 and set the paths
cargo build --locked --profile diag -p victory --features diagnostics --bins
cd tools\runkit
.\Run-Victory.ps1 -Fast -Book          # first run: builds pipelines as it goes, writes them down
.\Run-Victory.ps1 -Mode warm           # after any build or driver change: builds the used ones ahead
.\Run-Victory.ps1 -Fast -Book -Console series-s   # then play (the S profile fits the 8 GB card)
```

For an 8 GB card (settings.local.ps1 `$Defaults`, this PC): `VICTORY_CARD_ROOM_FOR_TARGETS_MB =
'1024'; VICTORY_CARD_WINDOWS_MB = '1024'; VICTORY_TEXTURE_BUDGET_MB = '1536'` (the buffer copies'
64 KiB / 1,536 MiB limits are the build's defaults for such cards since 2026-11-26).

## Quirks that cost time

- **Warm with `--used`.** `victory warm` alone builds every pipeline the title ships: 83,561, hours.
  `--used` builds those earlier `VICTORY_PIPELINE_BOOK=1` runs drew with: 1,932 in 264 s here (median
  303 ms each, p90 4.7 s, slowest 7.7 s; 6 refused). The run kit's `-Mode warm` passes `--used`.
- **Any change to the shader translator voids the pipeline cache.** The next run builds everything
  again in play (run 3: 0.1 fps for minutes). Warm after every such change. Changes elsewhere keep it.
- **One cache folder per tree.** The `nvidia` and `rtx-next` trees translate differently; give each
  its own `VICTORY_CACHE_DIR` (here two folders on a separate drive, about 600 MB each so far).
- **The pipeline book is written while the game runs**, so ending the process loses nothing.
- **Do not compile while playing.** A background `cargo` build made the intro hitch. If you must,
  `-j 4` and the compiler processes at Idle priority.
- **Windows PowerShell 5.1 and the runtime's log.** The runtime logs to stderr. Under
  `$ErrorActionPreference = 'Stop'`, `& victory.exe ... *> file` stops the script at the first line.
  Run it through `cmd /c "victory.exe ... > file 2>&1"` instead.
- **Right after `Stop-Process` the process still exists** for a second or two; wait for it before
  starting something that checks for it.
- **The console log wraps lines at 120 columns** when a run's output is redirected from PowerShell;
  grep for the start of a message, then read the next line.
- **The `nvidia` tree refuses to write inside the checkout** (`.victory-root`): run from the data
  folder. `rtx-next`'s `scripts\play.ps1` runs from the repository root and is fine.
- **Policy check**: `cargo xtask policy` (also the pre-commit hook). Files over 640 KiB are refused
  (the translator's `lower.rs` is over 520 KiB). `.bat` is not an allowed extension. Do not pipe it
  through `tail` in a `&&` chain: the failure is lost.
- **The view heap is capped at 1,000,000 on NVIDIA** (the title asks for about 1.25 million). Title
  slots 0-737,855, then 262,144 per-draw slots. Run 4 ran out of per-draw slots once.
- **Not implemented**: two pipeline-creation methods of the title's device (slots 0x218 and 0x1c8);
  logged 29 times at start, nothing visible yet.
- **Audio** comes in late on a first run (effects, then music, then dialogue): streaming, not decoding.
- **Commits** use the placeholder author `local <local@localhost>` and a `Co-Authored-By: Claude`
  trailer; each port names its source commit and author. Keep that.
- **No game material in the repository**, ever (`docs/asset-policy.md`); local paths go in the
  git-ignored `tools/runkit/settings.local.ps1`.

### Port 5 without twins, and the blue screen (2026-11-26, 19:18)

Upstream's port 5 reads a table's constant buffer through the raw twin of its slot; twins need
view heaps of 2^22 views and NVIDIA takes a million, so on this card the port fell back to copying
("not on 16 bytes", 6,000-6,700 a frame) and nothing changed. Commit 7eb5dc7 adds the no-twin
form (the slot itself a raw view, `constants_raw_in_slot`) and `VICTORY_PORT_CBV_COPY` (a table's
constant buffer in a copy read out of the copy, its range refreshed). Run 191409 with
`VICTORY_PORT_CBV=1`: the fill wait is gone (0.5-3.5 ms a frame, 9,106 constant buffers a frame
out of copies), **but the picture is all white** (constants read wrong: a null or misaligned raw
view, or a shader path that still reads the slot as a constant buffer), 2.5-3.5 fps with the
`victory-pipelines-N` threads building pipelines in play despite the warm, and four minutes in
the machine blue-screened: bugcheck 0x1A MEMORY_MANAGEMENT (0x41792, a corrupted page table
entry; minidump `C:\Windows\Minidump\112626-6828-01.dmp`). The runtime was at 25 GB private with
563 MB of RAM free and 1.2 GB of commit left (32 GB RAM, a 5 GB system-managed pagefile). The
layer has no path to a shutdown API; the crash is the kernel or a driver (the card reads 4 GB of
host copies in system memory) under that pressure, or RAM. Do not run `VICTORY_PORT_CBV=1` again
until the white picture is understood (a capture of one draw's descriptors would do it), and keep
commit headroom: a larger pagefile. The reboot at 17:33 the same day was Windows Update.

### Port 5 made to work on NVIDIA (2026-11-27, 00:00-00:30): the hair's colour was the constants

Commits 7eb5dc7 and a82a8f1. Without raw twins (NVIDIA's million-view heaps) the translator now
reads a table's or a picked constant buffer as raw bytes from its slot itself
(`constants_raw_in_slot`), the host makes every constant buffer's slot a raw view (dynamic
slots, in-place and copy-read ones, title slot mirrors, null ones), and `cbv_index` gives the
host's split index for raw reads too (that was the white picture). `VICTORY_PORT_CBV_COPY` (on
with the port) reads a table's constant buffer that lies in a card copy out of the copy, its
range refreshed for the submission. `VICTORY_PORT_CBV=1` is now in `settings.local.ps1`.

What it did: the window shot renders right at last: Lucia's hair dark brown with a rim light,
the orange INMATE shirt, a dim interior (scratchpad `cbv4`, `walk1`). The glowing orange hair
was the lighting constants read by the CPU fill before the title's buffer copy at its place in
the list had put them into the ring; read out of the copy on the card they come after it. So
"hair blonde/red" and probably "ray tracing through the hair" are closed by this, without the
builder. The ring fill's 60-80 ms wait is gone (8-9 ms a frame of flight waits remain, 3-4 small
fills). Frame rate unchanged at 9 fps in the window shot: the card runs 62-80 ms a frame and
idles 15-30 ms (a ~9.5 ms gap at each present while the last submission's refreshes run, and
~2.5 ms gaps where a fill waits for landings). Memory: the process is 26-27 GB private with 32 GB
of RAM; the user set a 16-32 GB pagefile after the 0x1A blue screen.

Tried after: `VICTORY_WIDE_THREADS=8` (no change: 95/108 ms frames against 97/108);
`VICTORY_FILL_TITLE=2` together with the port **removed the device 21 s in, at the 2D intro**
(run 201628): leave it off. After any translator change: warm with the same `-Set`, and the first
run still builds ~2,000 pipelines in play for four minutes (the D3D12 library was rejected
"created for a different version of the driver" after the Windows Update reboot and rebuilt by
the runs since; the warm's workers do not save the library, they only prime the driver's cache).
Scripted W/A keys do not move Lucia at the window; the stick mapping is there
(`gameinput.rs` `left_y`), so gameplay beyond the window needs a look at why.

## Next, in this order (from the user, 2026-11-27, 01:45)

The user's order: (1) skin and wall textures all or partially black, (2) 20 fps, (3) moving lips
during dialogue, (4) the stuttering intro animation. The state of each and where to start.

### Target 1: skin and wall textures all or partially black

**Lead, with evidence: the title writes block-compressed textures on the GPU and we drop those
writes.** Every run says "placed unordered texture view not mirrored (null: writes through it are
lost)" for 2,048-4,096+ views (printed at powers of two). Their shapes in run 004023:
BC7_UNORM (98) 1024x256 and 1024x512, BC7_UNORM_SRGB (99) 1024x1024, BC1 (71, 72) 256x256,
512x512, 1024x1024, 256x32, BC4 (80) 256x256, with mip chains: material textures (albedo, masks),
not render targets. The console lets a compute shader write a BC texture through an unordered
view (as a UINT alias); D3D12 does not, and `descriptor/host.rs` `make` gives such a slot a null
view (`View::Placed(true, ..)` of a texture, the `shortcut` comment). A texture never written
reads back as the title memory's bytes, which for GPU-produced textures is nothing: black. The
three computes that do not translate ("a lane mask exchanged where not every lane runs",
477b52e73ddeb6f2, 17af85f170e66f30, 8b437ab61d8008fe, 1.3 dispatches a frame each) also write
BC textures: the same producer, partly dropped twice.

- These views are not bound through per-draw tables: a diagnostic at the table path
  (`draw.rs` `note_placed_uav_writer`, this commit) saw none in a whole run. They are made for
  the title's own heap slots and picked by index, so the writer is found through the heap: log
  the dispatch that binds the heap and the slot's texture (`descriptor/host.rs` `make`, the slot
  index; the capture listing `VICTORY_CAPTURE` with `VICTORY_CAPTURE_WORLD` names dispatches
  and their tables).
- The fix: a host texture for such a title texture that the card can write. Two ways: (a) a
  placed resource pair on one heap, the BC texture and a `R32G32B32A32_UINT` (BC2/3/5/6/7:
  16-byte blocks) or `R32G32_UINT` (BC1/4) texture of width/4 x height/4 over the same bytes, the
  unordered view on the UINT one, an aliasing barrier between uses; (b) `CreateCommittedResource3`
  with castable formats (needs `ID3D12Device10` and `RelaxedFormatCastingSupported` in
  `D3D12_FEATURE_D3D12_OPTIONS12`: check it first; `host/place.rs` already asks OPTIONS12 for
  enhanced barriers). Then the host copy is the source of truth after a shader wrote it: the
  texture must not be uploaded again from title memory over the GPU's bytes (the texture queue's
  "changed after its upload with nothing to tell of it: uploaded again" path, 100-170 a run,
  would do exactly that), and `texback` must not try to tile it back ("a texture copied back
  could not be tiled e=Unsupported").
- Also check, cheaper first: `VICTORY_TEXTURE_BUDGET_MB=1536` (settings.local) caps the
  title-filled texture copies; the queue reports slots "waiting for their memory" (190 at the
  window shot, 7,500-17,000 while the world loads). Raise it to 2048 and see what the walls do;
  the copies' room is kept by `card_room_kept` now, so the cap may be unneeded.
- Done when: a close-up of Lucia's skin and the diner's walls show their textures in the first
  gameplay (the window shot and what follows), with no "not mirrored" warnings for BC formats.

### Target 2: 20 fps in gameplay

Where it stands (2026-11-27, the window shot, which is the first gameplay): 9 fps, 112 ms a
frame. The card runs 65-85 ms of it; with `VICTORY_SUBMIT_FRAMES=2` (the title may run one more
present ahead; not in settings.local, adds a frame of latency) the card's idle falls from 30 to
17 ms a frame but the frame did not shorten. 20 fps is 50 ms: the card's time must halve and
the title's own main thread, at 54% of its time in title code at 9 fps (profile `prof3`, about
60 ms of CPU a frame), must fit too, so some of that must be per-frame-fixed work that shrinks
with faster frames; measure it before promising 20.

The card's 80 ms by program (timing run 202827, `VICTORY_GPU_TIMING=1 VICTORY_D3D12_DRED=1
VICTORY_LATE_WAIT=0`): the FSR4 network 8e614708143ef917 11.6 ms in one dispatch (the title
asserts if its pipeline is refused; its "ACO" path needs `DeserializeComputePipelineStateX`,
which we answer E_NOTIMPL, so it takes the "NewBE" path; implementing the deserialise, that is
translating the console shader inside the serialised state, might give it other kernels);
`ped_skinning_cs` 5.5 ms in 233 dispatches of ~24 us (launch-bound: 32-thread groups, one
ped piece each); strand hair `smh_*` about 13 ms over ~20 programs; `compute_graph_ops` (the
facial rig) 5 ms in 331 dispatches of 3 us; our alias copies 2.4 ms in 78; then a tail of 1,300
kinds, ~8,000 commands, ~30 ms at 3-4 us each. The translator levers, in order of expected gain:
group doubling (two console groups of 32 in one host group of 64 for kernels without LDS or
group sync: Turing holds 16 groups per SM, so 32-thread groups run at half occupancy; `emit`
knows the group size from `native.threads`, the dispatch count is halved in `draw.rs`, indirect
dispatches need their count halved by the clamp dispatch); fewer, wider raw loads
(`rawBufferLoad` of one dword where the console loaded four: 45 of 147 in the rigid skinning
kernel); and the per-kernel work the console's scalar unit did cheaply (uniform loops reading a
table, 4,096 iterations in the rigid skinning kernel). On the CPU side the submit thread was
32% busy in the window shot (`send_late`, `submit`, `shadow::wide` refreshes), the eight list
threads 25-30% each, the relay 15%.

Measured and not worth repeating: barriers (`VICTORY_BARRIER_TEST=both`, no change), a fake GPU
load for the title's dynamic resolution (none), a 1080p display mode (the title keeps 4K
targets), `VICTORY_WIDE_THREADS=8` (none), `VICTORY_FILL_TITLE=2` with port 5 (device removed),
`VICTORY_PLACED_CUT=1` (hangs the title), the builder (`VICTORY_BVH_BUILDER=1`: right lighting,
5-6 fps, dark interiors, overlapping structures).

### Target 3: moving lips during dialogue

See "Target 2: mouths that move with the spoken lines" above (2026-11-26): the facial pipeline
is `ped_standard_head_cs` plus the `compute_graph_ops` rig solver, all dispatched every close-up
frame, translations exact; the audio clock is ruled out. Two things changed since: renderer
port 5 (constants are now read after the GPU writes them; the rig solver's constants may have
been stale too, so **look again first**: a line in the diner with the current build), and the
`sr_tracing` tiling pass now runs. Next step if still shut: read back the blend-shape output
(`970c4fbb3b017c0e`'s `Uav s0+48`, 1.4 MB) across two frames of a line; if it never changes, test
the graph ops on WARP one by one.

### Target 4: the stuttering intro animation

See "Target 1: the stutter in the intro animation" above: the thread-priority fix removed the
0.4-1.3 s freezes, the presents are clean (one 76 ms frame in the 15 s after the press in the
user's own run 151644), and the user still sees a stutter. Their hypothesis: stale draws showing
up a couple of milliseconds late (frames repeated or out of order, presents on time). The intro
after the press is a video uploaded as textures (12.7 MB a frame): a frame that misses its
upload shows the previous one. Measure the picture, not the presents: `holds.ps1` (the 256x256
patch hashed at ~60 Hz) across a press made the way the user makes it (three minutes after
launch, after the loading icon), and `VICTORY_VSYNC=1` as the first experiment (33.7-33.9 ms
frames on a 59 Hz display without it). Note that the user presses A when the loading icon has
gone, minutes after launch, not at 14 s as `VICTORY_KEYS=14:Enter` does.

### The build and settings to start from

`rtx-next` at this commit; `tools/runkit/settings.local.ps1` on this PC:
`VICTORY_CARD_ROOM_FOR_TARGETS_MB=1024 VICTORY_CARD_ROOM_FOR_BUFFERS=1 VICTORY_LAZY_BACK=1
VICTORY_CARD_BUFFERS_MIN_KB=64 VICTORY_CARD_BUFFERS_MB=1536 VICTORY_CARD_WINDOWS_MB=1024
VICTORY_TEXTURE_BUDGET_MB=1536 VICTORY_PORT_CBV=1`. After a translator change: `-Mode warm` with
the same `-Set`, and the first run builds pipelines in play for about four minutes (the warm
primes the driver's cache, not the library). The machine: 32 GB of RAM, the process 26-27 GB
private in the world, a 16-32 GB pagefile set by the user on 2026-11-27 after the 0x1A blue
screen; keep an eye on "the PC has N MB free" in the `memory:` line.

## Target 1, re-examined (2026-11-26, 21:00-22:00, runs 211202-215xxx)

**The lead above was a misread.** Every "placed unordered texture view not mirrored" line came from the
`View::Placed` arm of `descriptor/host.rs` `make_twinned`, which sent any placed *texture* view that
`placed_texture()` could not resolve into the unordered shortcut, flag or no flag. Those views were the
title's own null placeholders: shader views (device method 0x230) at address 0, levels 15, in whatever
format the slot had held (the lines always said `address=0x0`). A counter on the device methods now shows
what the title asks for: through the first gameplay, 68 placed *buffer* unordered views and **0 texture
ones** (`asked=[68, 0]` in the refusal line). The arm now checks the flag; the null shader views are
counted and logged as "placed shader view of a texture the host has no resource for (null)".

What is in the tree anyway, and stays (harmless, `VICTORY_BC_WRITES=0` turns it off):
- `resource.rs` `block_alias`: BC1/BC4 -> R32G32_UINT, BC2/3/5/6/7 -> R32G32B32A32_UINT. Title-filled 2D
  BC host copies are made castable to that format with unordered access (`CreatePlacedResource2` in
  `pool.rs`, `CreateCommittedResource3` in `host_texture_as`; the card reports
  `RelaxedFormatCastingSupported`, inbox D3D12Core 10.0.26100.9278; spec: a cast view of a compressed
  resource has one level, which a UAV is). `HostTexture::alias` and `title_filled()` (the title's flags,
  not the host's: the checks in `draw.rs`, `list.rs`, `ahead.rs` that meant "the title fills it" now ask
  that). `descriptor/host.rs` `placed_texture_uav` makes such a view (the words are SRV-shaped: word 4 is
  the level); `texture.rs` `placed_uav_made` / `card_writers_bound` (from `draw.rs` `bind`, compute) upload
  the texture once more ahead of the dispatch, mark it `host_written`, move it to the unordered state.
- **Pitfall, cost a run:** `GetCopyableFootprints` and `GetResourceAllocationInfo` refuse a BC description
  with the unordered flag (they know nothing of castable formats): every byte count came back `u64::MAX`,
  no BC texture uploaded, the texture accounting let everything go. `resource::query_desc` strips the
  flag for every such query (`host_bytes`, `staging`, `alias`, `zeros`, `pool`, `standin`, `texback`,
  `capture`, `book`).
- `VICTORY_SHADER_SAVE_REFUSED=<dir>` (`pipeline.rs`): the containers of the shaders that do not
  translate, for `victory translate --list --jobs 1 <file.cdxil>` offline (set `VICTORY_HOST_WAVE=32
  VICTORY_WAVE_WHY=1`). While the game runs it locks `target\diag\victory.exe`: build with
  `--target-dir target/tr` for the tools and copy the exe over afterwards.

**The three refused computes translate now** (477b52e73ddeb6f2, 17af85f170e66f30, 8b437ab61d8008fe:
INT8 dot-product networks, `v_dot4c_i32_i8`, groups of 64, that store 16-byte blocks with
`buffer_store_dwordx4`; they are GPU block compressors writing *buffers*, which is the texture's memory
for all the host knows). Two changes in `victory-shader` `emit/lower.rs`: `scc_some` answers per host
wave in the local forms (`local_any`/`local_ok`; the refusal at 0x218 was a spin-wait "loop while some
lane's flag is still unset", which each half waiting for its own lanes is exactly), and the constant
lane-mask select no longer refuses in the local form (`console_lane` is known there). They come out in the
"local waves (APPROXIMATE, any mask), without early-outs" form: each half of 32 reads its own lane 0 and
lane 31 for the console's 0 and 63 (a `v_permlanex16` + `v_readlane 0/63` reduction). Bound in run 214346
without a device loss; whether their blocks come out right is not seen yet (see below). The shader crate's
tests: 293 pass; 3 hardware tests failed only because the game held the card ("device suspended").

**Where the black textures stand now.** The computes write their blocks into buffers on the card;
`shadow.rs` `gpu_writes` notes "another host copy of the same bytes (a texture in it) is not told". The
texture host copy is filled from title memory, which gets the card's bytes only through the copy-back
(`copy_back_with` -> `put_changed` -> `watch::landing` -> `writing` -> `descriptor::host::remapped`, which
does make the placed texture slots there due again). A probe in `placed_texture` counts and logs
"a placed texture lies in buffer memory shaders wrote" (`PLACED_IN_WRITTEN`). Next: read that count in
the window shot; if textures do lie in written buffers, check that their slots are re-uploaded after the
landing (the heap pass's "changed after its upload" path), and that the landing happens for those
buffers at all (table-bound unordered buffer views of dispatches; `VICTORY_WRITE_SKIP`). If no texture
lies in written memory, the black skin and walls have another cause (the texture budget cap
`VICTORY_TEXTURE_BUDGET_MB`, slots "waiting for their memory", or the materials' other inputs).

## Black skin and walls are late textures; the frame rate's shape (2026-11-26, 22:00-23:15)

**What the black textures are.** Run 214949's shots: at the window shot (491 s) the whole diner
interior was black; at 856 s it was textured. The log at 491 s: "passes that made none for lack of
room: 7650", title-filled copies 1,583 MB at the 1,536 MB cap (`VICTORY_TEXTURE_BUDGET_MB`: with it set,
`resource::room()` is `filled < limit` and nothing else), the queue 19,744 slots with 1,677 due, the card
6.76 of 7.21 GB. Copies are let go only when no slot views them, so a cutscene's textures stayed for
minutes. The user's shots of the club scene: faces and hands black while the shirt is lit, then fine
seconds later (the handoff's run-4 row said the same: "Lucia's face and skin missing" with "the card has
no room"). A rainbow-noise forearm is a copy cut from a pooled block showing the block's old bytes
before its first upload; a shirt fading to black is a texture uploaded in part (the unmapped large
level uploads zeros). `settings.local.ps1` now has `VICTORY_TEXTURE_BUDGET_MB=2048` (runs 220752 on:
room-less passes 7,650 -> 1,324, fps unchanged, the card right at its budget). Not done: a slot should
show nothing (or the levels that are there) rather than stale bytes; `VICTORY_LEVELS_ON_DEMAND=1`
(`tiles.rs`: levels given memory as the title holds them, "no level is shown before it is written
whole") is the untried switch for exactly that, and it also frees the memory of unheld levels.

**The frame rate's shape (timing runs 222355 and 223952, `VICTORY_GPU_TIMING_TOP=400` new).** At the
window shot the card runs 68-70 ms of commands a frame (the normal runs: 94 ms frames, the card 96-98%
busy, the submit thread 32-40 ms preparing and 18 ms copying back, waiting 16-22 ms for the card). The
400 kinds listed hold 64 ms in 6,341 commands: top 40 = 32 ms, 41-100 = 16, 101-200 = 10, 201-400 = 7;
no kind above 4.6 ms. By family: `ped_skinning_cs` 8.6 ms in 477 dispatches, `compute_graph_ops` 6.2
in 1,085, strand hair (`smh_*`) about 10, our alias copies 2.4 in 78 (R32_UINT views of R9G9B9E5
targets the host renders as RGBA16F), decal classification 3.8 in one, scattering 2.8, mesh-shader
draws about 5, `indirect` executions 19-23 ms in 4,500 (5 us each), the FSR4 network only 0.14 ms at
this shot on Series S. Guards: 627 faults a frame on title threads, 19.5 ms waiting for the guards
and 8.7 ms changing protections (all threads). The timeline of a normal run has the title's own thread
"outside our spans" for 68-90 ms of each frame on the present's chain, scaling with the frame: it spins
on the card. `VICTORY_BARRIER_TEST=both` no longer runs (a list fails to close: parameter incorrect).

**Belobug / upstream, at the user's request (22:45).** belobug/rx9070xt = upstream xtx/main c9c932c +
3 commits; their README: 45 ms (22 fps) on a 7900 XTX, Series S profile, renderer ports 1, 2, 5, 6, 7,
the card 36 ms there. A full merge into rtx-next conflicts in 20 files (49 hunks); decided with the
user: cherry-pick, not merge. Done in `victory_repo/victory-merge` (branch `port6-try` on 302d660):
port 6 (`VICTORY_PORT_GUARD=1`, the guards' lock shared by faults and refreshes; six commits, conflicts
in watch.rs resolved, port 2's call dropped) and the output-size patch (`VICTORY_OUTPUT=1920x1080`,
`victory-loader/src/resolution.rs`, three commits; `Patched` enum of its own since we lack
`frame_rate.rs`). The title's output size comes from its settings file's `resolutionFullscreen` matched
against its own mode table (`VICTORY_DISPLAY_MODE` alone changes nothing); editing the file to
1920x1080 changed nothing here either (run 225148: 4K targets, 10-11 fps), and the patch alone (run
230xxx, "patched site=0x224352b ... 1920x1080") still left 2 x 3840x2160 back buffers and one 4K depth
target, 10.5-11.5 fps. Run 13 tries the patch together with `VICTORY_DISPLAY_MODE=1920x1080`.

## Sources and credit (2026-11-26, late)

Work taken from, or pointed to by, other trees, with where it came from:

- **avatarmanz/VIctory-7900XTX** (upstream, remote `xtx`): renderer ports 5 (`VICTORY_PORT_CBV`,
  3920dc2, transplanted earlier), 6 (`VICTORY_PORT_GUARD`, fc8ac6c and five commits after it, now
  cherry-picked on `rtx-next-experiments`, authors kept), the output-size and render-scale switches
  (`victory-loader/src/resolution.rs`, e151e71 45cad4c 4d7185e, the same branch), the 20 translator
  commits transplanted on 2026-11-26, levels on demand (`tiles.rs`, with AscilCH's pull request #747
  fixes), and the light stand-in's idea, which upstream credits to beloooo (VIctory-Belobug-AMD).
- **Belobug/VIctory-Belobug-AMD** (remote `belobug`): the measured README (22 fps on a 7900 XTX with
  ports 1, 2, 5, 6, 7; Series S; the output size's 16 ms) that set tonight's order of work; their
  branch `rx9070xt` is upstream c9c932c plus three commits (RX 9070 XT fixes and a HUD, title textures
  uploaded in the prelude, a PC FidelityFX 4K path).
- **codec-d/cool_fox**, carried on by **DoubleGhosty/cool_fox** (remote `cf-dg`; the original
  repository is gone): the texture guard's part in the garbage skins (`VICTORY_TEXTURE_GUARD=0`, their
  launcher and commit 0c2ece7), the hair-strand program list and match now behind our opt-in
  `VICTORY_SKIP_HAIR=1` (f4164f8 `hair_strands`), the rayless screen-probe skip (same commit), the
  six one-wave computes forced to wave 32 by hash (3c1f8bd), the zero-read recheck of partly mapped
  textures (c341046, reverted there), and their 8 GB NVIDIA launcher defaults (levels on demand on,
  `VICTORY_THIN=0 VICTORY_JOIN_PARTS=0 VICTORY_LIBRARY_STORE_LATER=1 VICTORY_PIPELINE_WAIT_MS=0`, Series X
  profile). Upstream had already taken cool_fox's AMD-safe parts (a12870a: the two-plane compose this
  tree's overlay fix follows, TablePlan, the per-buffer barrier, residency behind switches).
- Other forks seen, not yet read: miaohaixiang1-netizen/cool-fox-12gb (`VICTORY_TEXTURE_DROP_MIPS`:
  the largest levels left out so an 8 GB card keeps the world resident), Zarina35/victory-coolfox-nvidia
  (NVIDIA fixes, a performance overlay, a Resizable BAR setting), stylexss/VIctory-cool-fox.

Commit messages and doc comments name the source commit wherever a piece was taken.

**Runs 12-15 (23:00-00:05), one switch each at the window shot (Series S, budget 2048):**

| Run | Switch | fps | What changed |
|---|---|---|---|
| 230447 | `VICTORY_OUTPUT=1920x1080` (patch applied) | 10.5-11.5 | 4K back buffers and depth stay |
| 231431 | + `VICTORY_DISPLAY_MODE=1920x1080` | 10-11 | same |
| 233428 | `VICTORY_LEVELS_ON_DEMAND=1` (tier 4) | 10-11 | worse: interior black at 490 s, card 7.8 of 7.2 GB, 9,663 room-less passes |
| 235305 | `VICTORY_TEXTURE_GUARD=0` (cool_fox) | 10.3-11.9 | interior fully textured at 519 s; textures 168 MB a frame re-uploaded (14-36 before); preparing 44.6 ms (+12 on the submit thread) |

The texture guard off is the one texture lever that showed: changed textures are re-uploaded
promptly (cool_fox saw it cure the rainbow forearms and black faces). Its cost is CPU on the submit
path, invisible while the card is the wall; once the card is faster it will show. Levels on demand
is not usable here as it stands. The output-size patch does nothing on the Series S profile: run 16
tries it on Series X.
