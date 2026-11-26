# Copy to settings.local.ps1 (not committed) and set your own paths.
# Every path must be outside the source checkout.

# Your extracted game install (the folder with game_scarlett_master.exe).
$GameDir = 'D:\Games\your-game-install'

# Saves, per-run state and logs.
$DataDir = Join-Path $env:LOCALAPPDATA 'victory-runkit'

# Shader and pipeline cache: about 0.6 GB of translations, plus the driver's
# pipelines, which grow with play. A big, fast drive is best.
$CacheDir = 'D:\victory-cache'

# Switches every run gets (Run-Victory.ps1 -Set adds or overrides per run),
# e.g. @{ VICTORY_AMBIENT = 'off' }.
# Do not lower the per-draw view slots on NVIDIA (VICTORY_DYNAMIC_SLOTS): in a
# 16-minute run they peaked at 204,014 of 262,144 while the game used 737,853
# of its 737,855; the 1,000,000-view heap is full either way.
$Defaults = @{}
