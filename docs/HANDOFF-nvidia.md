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

How to get there quickly: the first cutscene with dialogue (the prison yard into the diner
flashback). Use `Drive-Run.ps1 -EnterAfter` to skip the menus, and capture a few hundred frames
in which a line is spoken. Done when: the mouths move in time with the lines in that cutscene.

## How to run

```powershell
# once: copy tools\runkit\settings.example.ps1 to settings.local.ps1 and set the paths
cargo build --locked --profile diag -p victory --features diagnostics --bins
cd tools\runkit
.\Run-Victory.ps1 -Fast -Book          # first run: builds pipelines as it goes, writes them down
.\Run-Victory.ps1 -Mode warm           # after any build or driver change: builds the used ones ahead
.\Run-Victory.ps1 -Fast -Book          # then play
```

For an 8 GB card add, until step 5 is done (settings.local.ps1 `$Defaults`):
`VICTORY_CARD_ROOM_FOR_TARGETS_MB = '1024'; VICTORY_CARD_ROOM_FOR_BUFFERS = '0'`.

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
