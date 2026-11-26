<#
.SYNOPSIS
  Runs the VIctory runtime (diagnostics build) against your own game install,
  with NVIDIA/D3D12 debugging switches, and summarizes the log afterwards.

  Where things are comes from settings.local.ps1 next to this script (not
  committed; copy settings.example.ps1), else from the environment:
    $GameDir   your extracted game install (VICTORY_GAME_DIR)
    $DataDir   saves and logs              (default %LOCALAPPDATA%\victory-runkit)
    $CacheDir  shader and pipeline cache   (default $DataDir\cache)
    $Defaults  switches every run gets, e.g. @{ VICTORY_AMBIENT = 'off' }
  All of them must be outside the source checkout; the runtime refuses otherwise.

.EXAMPLE
  .\Run-Victory.ps1 -Mode check          # config, executable imports, nothing runs
  .\Run-Victory.ps1 -Mode shaders        # translate every shader ahead of time (minutes)
  .\Run-Victory.ps1 -Fast -Book          # play with scripts\play.ps1's settings; pipelines written down
  .\Run-Victory.ps1 -Mode warm           # host pipelines earlier -Book runs drew with, made ahead
  .\Run-Victory.ps1                      # play
  .\Run-Victory.ps1 -D3dDebug -Dred      # play with the D3D12 debug layer and DRED
  .\Run-Victory.ps1 -AudioDebug          # per-voice audio decoder logging
  .\Run-Victory.ps1 -Set @{ VICTORY_AMBIENT = 'off' }
#>
[CmdletBinding()]
param(
    [ValidateSet('check', 'shaders', 'warm', 'run')] [string] $Mode = 'run',
    [string] $Game,
    [switch] $D3dDebug,
    [switch] $Gbv,
    [switch] $Dred,
    [switch] $WaveWhy,
    [switch] $AudioDebug,
    [switch] $Warp,
    [switch] $Fresh,
    [switch] $Release,
    [switch] $Fast,
    [switch] $Book,
    [ValidateSet('series-x', 'series-s')] [string] $Console = 'series-x',
    [hashtable] $Set = @{}
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$GameDir = $env:VICTORY_GAME_DIR
$DataDir = Join-Path $env:LOCALAPPDATA 'victory-runkit'
$CacheDir = $null
$Defaults = @{}
$local = Join-Path $PSScriptRoot 'settings.local.ps1'
if (Test-Path -LiteralPath $local) { . $local }
if (!$CacheDir) { $CacheDir = Join-Path $DataDir 'cache' }
$logs = Join-Path $DataDir 'logs'

$profileDir = if ($Release) { 'release' } else { 'diag' }
$runtime = Join-Path $repo "target\$profileDir\victory.exe"
if (!(Test-Path -LiteralPath $runtime)) {
    throw "No runtime at $runtime. Build it: cargo build --locked --profile diag -p victory --features diagnostics --bins"
}
if (!$Game -and !$GameDir) { throw 'Set $GameDir in settings.local.ps1 (or VICTORY_GAME_DIR), or pass -Game.' }
New-Item -ItemType Directory -Force $logs, $CacheDir | Out-Null

# The game executable: given, or the one .exe at the top of the game folder
# (the Series X one when there are several).
if (!$Game) {
    $exes = @(Get-ChildItem -LiteralPath $GameDir -Filter *.exe -File -ErrorAction SilentlyContinue)
    $pick = @($exes | Where-Object { $_.Name -match 'scarlett' })
    if ($pick.Count -eq 1) { $Game = $pick[0].FullName }
    elseif ($exes.Count -eq 1) { $Game = $exes[0].FullName }
    else {
        $names = ($exes | ForEach-Object { $_.Name }) -join ', '
        throw "Pass -Game. Executables at the top of ${GameDir}: $names"
    }
}
if (!$GameDir) { $GameDir = Split-Path -Parent $Game }

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$log = Join-Path $logs "$Mode-$stamp.log"
$out = Join-Path $logs "$Mode-$stamp.out.log"

Remove-Item env:VICTORY_CONFIG -ErrorAction SilentlyContinue
$env:NO_COLOR = '1'
$env:VICTORY_GAME_DIR = $GameDir
$env:VICTORY_CACHE_DIR = $CacheDir
$env:VICTORY_DATA_DIR = Join-Path $DataDir 'state'
if ($Fresh) { $env:VICTORY_DATA_DIR = Join-Path $DataDir "state-$stamp" }
if ($AudioDebug) { $env:VICTORY_LOG = 'info,victory_hle::modules::apu=debug,victory_hle::modules::audio=debug' }
if (!$env:VICTORY_LOG) { $env:VICTORY_LOG = 'info' }
$switches = @{}
# -Fast: what scripts\play.ps1 sets without -Safe (README, "How to run it");
# keep in step with it.
if ($Fast) {
    $play = @{
        VICTORY_TEXTURE_NO_WAIT = '1'; VICTORY_RELAY = '2'; VICTORY_WIDE_BACK = '1'; VICTORY_CARD_REHASH_RUNS = '1'
        VICTORY_JOIN_COPIES = '1'; VICTORY_INDIRECT_WINDOW = '1'; VICTORY_RELAY_COPY = '1'; VICTORY_CARD_STEPS = '2'
        VICTORY_INDIRECT_BATCH = '2'; VICTORY_MAP_THREADS = '1'; VICTORY_TITLE_PRIORITIES = '0'; VICTORY_CARD_AWAKE = '1'
        VICTORY_SHOW_AFTER = '1'; VICTORY_AMBIENT = '0.8'; VICTORY_D3D12_DRED = 'light'; VICTORY_SKIP_PIPELINES = 'none'
        VICTORY_THIN = '1'; VICTORY_JOIN_PARTS = '8'; VICTORY_UPLOAD_AHEAD = '1'
    }
    foreach ($k in $play.Keys) { $switches[$k] = $play[$k] }
}
# -Book: the pipelines this run makes are written down for -Mode warm.
if ($Book) { $switches.VICTORY_PIPELINE_BOOK = '1' }
foreach ($k in $Defaults.Keys) { $switches[$k] = [string]$Defaults[$k] }
if ($D3dDebug -or $Gbv) { $switches.VICTORY_D3D12_DEBUG = '1' }
if ($Gbv) { $switches.VICTORY_D3D12_GBV = '1' }
if ($Dred) { $switches.VICTORY_D3D12_DRED = '1' }
if ($WaveWhy) { $switches.VICTORY_WAVE_WHY = '1' }
if ($Warp) { $switches.VICTORY_D3D12_WARP = '1' }
foreach ($k in $Set.Keys) { $switches[$k] = [string]$Set[$k] }
foreach ($k in $switches.Keys) { Set-Item -Path "env:$k" -Value $switches[$k] }

@(
    "mode     $Mode",
    "runtime  $runtime",
    "game     $Game",
    "data     $env:VICTORY_DATA_DIR",
    "cache    $env:VICTORY_CACHE_DIR",
    "switches " + (($switches.GetEnumerator() | Sort-Object Name | ForEach-Object { "$($_.Name)=$($_.Value)" }) -join ' ')
) | ForEach-Object { Write-Host $_ }
Set-Content -LiteralPath (Join-Path $logs 'latest.txt') -Value $log -Encoding utf8

$arguments = switch ($Mode) {
    'check' { @('inspect', "`"$Game`"", '--imports') }
    'shaders' { @('shaders', "`"$GameDir`"") }
    'warm' { @('warm', '--used') }
    'run' { @('run', "`"$Game`"", '--execute', '--console', $Console, '--unimplemented', 'trap') }
}
if ($Mode -eq 'check') {
    Push-Location $DataDir
    try { & $runtime config } finally { Pop-Location }
}
$started = Get-Date
Write-Host "log      $log"
# (run from the data folder: outside the checkout, as the runtime requires)
$p = Start-Process -FilePath $runtime -ArgumentList $arguments -WorkingDirectory $DataDir `
    -NoNewWindow -Wait -PassThru -RedirectStandardError $log -RedirectStandardOutput $out
Write-Host ("exit     {0} after {1:N0} s" -f $p.ExitCode, ((Get-Date) - $started).TotalSeconds)

& (Join-Path $PSScriptRoot 'Summarize-VictoryLog.ps1') -Log $log -Since $started -DataDir $env:VICTORY_DATA_DIR
