# Copy to settings.local.ps1 (not committed) and set your own paths.
# Every path must be outside the source checkout.

# Your extracted game install (the folder with game_scarlett_master.exe).
$GameDir = 'D:\Games\your-game-install'

# Saves, per-run state and logs.
$DataDir = Join-Path $env:LOCALAPPDATA 'victory-runkit'

# Shader and pipeline cache: about 0.6 GB of translations, plus the driver's
# pipelines, which grow with play. A big, fast drive is best.
$CacheDir = 'D:\victory-cache'

# Switches every run gets (Run-Victory.ps1 -Set adds or overrides per run).
# On NVIDIA: the per-draw descriptor slots peaked near 70,000 of 262,144 while
# the game's own slots nearly filled the 1,000,000-view heap, so fewer
# per-draw slots leave the game more room.
$Defaults = @{
    VICTORY_DYNAMIC_VIEWS = '131072'
}
