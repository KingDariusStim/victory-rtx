# Handoff: the NVIDIA work (2026-11-25)

Where the NVIDIA effort stands, what was measured, what is broken, what to do next, and the
quirks that cost time. Written for whoever picks this up next, human or agent. Everything here
was measured on one PC; numbers from other machines are labelled.

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
3. **Lazy read-back** (the largest frame-rate lever left). Do not fetch every buffer shaders wrote
   back into title memory each frame. Protect the title's copy, and fetch a page from the card only
   when the title's CPU code touches it (a fault). Buffers the title never reads on the CPU (most:
   lighting, particles, visibility) then cost nothing. The guard-and-fault machinery already exists
   for the other direction (`crates/victory-gpu/src/watch.rs`).
4. **Retire buffers that stopped changing.** One that came back unchanged N frames running is no
   longer compared until new GPU work writes it. Smaller gain; easy.
5. **Size the reserves from the budget in code**, not by environment: `targets_room_kept()`
   (`resource.rs`, 3 GB default) and `card_room_kept()` (`crates/victory-gpu/src/shadow.rs`, 1 GB).
   For example a seventh and a twentieth of the budget, which keeps 3 GB and 1 GB on a 24 GB card.
   Worth offering to avatarmanz upstream.
6. **Convert R9G9B9E5 stand-in copies** in `standin.rs` (format 67 <-> RGBA16F), then see whether the
   blue sky goes. Resolves between stand-ins are also still skipped (counted in the report line).
7. **Vsync option**: `Present(1, 0)` (or 2 at 30 fps) behind a switch.
8. **Temporal ghosting**: find the pass writing motion vectors and check it runs (dropped draws,
   refused pipelines); a switch that drops TAA history would confirm.
9. **The six refused wave-64 programs** (problem 6).
10. Warm the full pipeline set once, overnight: `victory warm` without `--used` (83,561 pipelines).
11. Bigger: stream textures by tiles (partial residency; `crates/victory-gpu/src/tiles.rs` is a start).

Done and ruled out: Resizable BAR (no official support on RTX 20 cards; only a firmware mod);
a RAM disk as a cache (the copies are already RAM to RAM; the cost is waiting and comparing).

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
