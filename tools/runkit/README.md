# Run kit (Windows, NVIDIA-tested)

PowerShell scripts that run the diagnostics build against your own game install,
with D3D12 debugging switches, and summarize each run's log. Tested on an RTX 2080 SUPER
(8 GB, driver 610.88, Windows 11).

## Setup

1. Build the runtime from the repository root:
   `.\Initialize-FormatTables.ps1` then
   `cargo build --locked --profile diag -p victory --features diagnostics --bins`
2. Copy `settings.example.ps1` to `settings.local.ps1` (git-ignored) and set `$GameDir`,
   `$DataDir`, `$CacheDir`. All three must be outside the checkout.
3. Optional, recommended on slow first runs: raise the GPU timeout from an elevated
   PowerShell, then reboot: `.\Set-GpuTimeout.ps1` (undo: `-Undo`). Windows resets the
   driver when one GPU submission runs over 2 s, which this runtime can do at 1-4 fps.
4. Optional: NVIDIA Control Panel > Shader Cache Size > Unlimited.

## Runs, in order

| Step | Command | Notes |
|---|---|---|
| Check | `.\Run-Victory.ps1 -Mode check` | reads the executable and its imports; nothing runs |
| Warm | `.\Run-Victory.ps1 -Mode warm -WaveWhy` | about 12 min the first time: ~25,700 shaders translated, compute pipelines made |
| Play | `.\Run-Victory.ps1 -D3dDebug -Dred` | first run builds graphics pipelines as they come (very slow); later runs load them |

First-boot screens need a controller or the keyboard (WASD/IJKL sticks, Enter=A,
Backspace=B, Esc=Menu). Close the window to end a run; the summary prints after.
`.\Summarize-VictoryLog.ps1` summarizes the latest log again (or `-Log <file>`).

## Switches

| Switch | Effect |
|---|---|
| `-D3dDebug` | D3D12 debug layer; errors and warnings logged as they happen (`D3D12:` lines) |
| `-Gbv` | plus GPU-based validation (much slower) |
| `-Dred` | on a GPU fault, the command and address that faulted |
| `-WaveWhy` | why console 64-lane programs keep 64-lane waves or fall back (NVIDIA runs 32) |
| `-AudioDebug` | per-voice XMA and Opus decoder logging |
| `-Fresh` | a new save/state folder (first-boot screens again) |
| `-Warp` | software renderer (very slow) |
| `-Release` | `target\release` instead of the diagnostics build |
| `-Set @{ NAME = 'value' }` | any other `VICTORY_*` switch for this run |

## First results on an RTX 2080 SUPER (2026-11-25)

- Warm: 25,745 of 25,758 shaders translated (13 refused: 10 pixel, 3 compute); 2,700 compute
  pipelines made, 31 refused by the driver because they need 64-lane waves.
- Play: boots, menus at 30 fps, opening cutscene and gameplay reached at 1-2 fps on the first
  run, while graphics pipelines are built. Audio works (music and dialogue arrive late while
  streaming is slow). Areas lit only by bounce light are black: the game's ray-traced indirect
  light does not run yet.
- The game's view heap is 1,245,184 slots; NVIDIA allows 1,000,000. In a 16-minute run the
  game used 737,853 of the 737,855 slots it gets and the runtime's per-draw slots peaked at
  204,014 of 262,144: the heap is full on both sides, so moving the split does not help. The fix
  is remapping the game's slots onto the host heap (open work).
- That run ended in `DXGI_ERROR_DEVICE_HUNG` (the GPU timeout) when a +100 MHz overclock was
  applied mid-run. Test at stock clocks, so overclock instability is not mistaken for a runtime bug.
