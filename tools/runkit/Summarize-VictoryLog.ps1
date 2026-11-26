<#
.SYNOPSIS
  Summarizes a VIctory runtime log: exit, frame rate, GPU failures, D3D12
  debug-layer messages, NVIDIA-relevant counters, the most frequent warnings,
  and any crash report written during the run.

.EXAMPLE
  .\Summarize-VictoryLog.ps1                 # the latest log
  .\Summarize-VictoryLog.ps1 -Log path\to\run.log -Top 25
#>
[CmdletBinding()]
param(
    [string] $Log,
    [datetime] $Since = [datetime]::MinValue,
    [string] $DataDir,
    [int] $Top = 12
)

$ErrorActionPreference = 'Stop'
# Where runs keep their logs and state: settings.local.ps1, as Run-Victory.ps1.
$runkitData = Join-Path $env:LOCALAPPDATA 'victory-runkit'
$local = Join-Path $PSScriptRoot 'settings.local.ps1'
if (Test-Path -LiteralPath $local) {
    $saved = $DataDir
    . $local
    if ($DataDir -and !$saved) { $runkitData = $DataDir }
    $DataDir = $saved
}
$logs = Join-Path $runkitData 'logs'
if (!$Log) { $Log = (Get-Content -LiteralPath (Join-Path $logs 'latest.txt') -TotalCount 1).Trim() }
if (!$DataDir) { $DataDir = Join-Path $runkitData 'state' }

$esc = [char]27
$lines = Get-Content -LiteralPath $Log | ForEach-Object { $_ -replace "$esc\[[0-9;]*m", '' }

function Write-Section([string] $title) { Write-Host ''; Write-Host "== $title" }
function Select-Last([string] $pattern) { $lines | Where-Object { $_ -match $pattern } | Select-Object -Last 1 }
# The message of a tracing line: after "LEVEL target: "; with -Fold, numbers
# replaced by # so that repeats group.
function Get-Message([string] $line, [switch] $Fold) {
    $m = $line -replace '^\S+\s+(TRACE|DEBUG|INFO|WARN|ERROR)\s+[\w:]+:\s*', ''
    if ($Fold) { $m = $m -replace '0x[0-9a-fA-F]+', '0x#' -replace '\b\d+(\.\d+)?\b', '#' }
    if ($Fold -and $m.Length -gt 160) { $m = $m.Substring(0, 160) + '...' }
    $m
}

Write-Host "log: $Log ($($lines.Count) lines)"

Write-Section 'Exit'
$exit = Select-Last 'runtime exit: code|VIctory exited'
if ($exit) { Write-Host $exit } else { Write-Host '(no exit line: still running, killed, or crashed hard)' }

Write-Section 'Host GPU'
foreach ($p in 'host wave lanes', 'host view heaps hold', 'host D3D12 device ready', 'host device on the software adapter', 'GPU-based validation on', 'does not render to this format') {
    $l = Select-Last $p
    if ($l) { Write-Host (Get-Message $l) }
}

Write-Section 'Frame rate (periodic stats)'
$fps = @($lines | Where-Object { $_ -match '(\d+(\.\d+)?) fps, (\d+(\.\d+)?) ms/frame' } | ForEach-Object {
        if ($_ -match '(\d+(\.\d+)?) fps, (\d+(\.\d+)?) ms/frame') { [double]$Matches[1] }
    })
if ($fps.Count) {
    $sorted = $fps | Sort-Object
    Write-Host ("{0} samples: min {1:N1}  median {2:N1}  max {3:N1}  last {4:N1} fps" -f $fps.Count, $sorted[0], $sorted[[int]($sorted.Count / 2)], $sorted[-1], $fps[-1])
    Write-Host (Get-Message (Select-Last 'ms/frame')).Substring(0, [Math]::Min(400, (Get-Message (Select-Last 'ms/frame')).Length))
} else { Write-Host '(none)' }
$world = Select-Last 'the first frame of 500 draws'
if ($world) { Write-Host "world reached: $world" }

Write-Section 'NVIDIA: 16-bit colour stand-ins'
$s = Select-Last '16-bit colour targets in 8-bit RGBA stand-ins'
if ($s) { Write-Host (Get-Message $s) } else { Write-Host '(no stand-in targets made, or no stats line yet)' }
$skip = Select-Last 'copy between a stand-in target and title-format texels skipped'
if ($skip) { Write-Host "first skipped copy: $skip" }

Write-Section 'NVIDIA: descriptor heap'
$use = Select-Last 'view heap use: '
if ($use) { Write-Host (Get-Message $use) }
foreach ($p in 'no free dynamic descriptor slots', 'title slot past the host heap', 'host view heap capped at', 'no host descriptor heap', 'no host view heap') {
    $n = @($lines | Where-Object { $_ -match $p }).Count
    if ($n) { Write-Host "$n x '$p' - last: $(Select-Last $p)" }
}

Write-Section 'NVIDIA: 64-lane shaders on 32-lane waves'
$why = @($lines | Where-Object { $_ -match 'WAVE64-WHY' })
if ($why.Count) {
    $why | ForEach-Object { if ($_ -match '\[(.*)\]') { $Matches[1] -split ', ' } } | Group-Object | Sort-Object Count -Descending |
        ForEach-Object { Write-Host ("{0,6} x {1}" -f $_.Count, $_.Name) }
    Write-Host "($($why.Count) programs kept 64-lane waves)"
} else { Write-Host '(run with -WaveWhy to list why programs keep 64-lane waves)' }
$approx = @($lines | Where-Object { $_ -match 'APPROXIMATE' }).Count
if ($approx) { Write-Host "$approx shader translations used an APPROXIMATE wave form" }

Write-Section 'Audio'
$a = Select-Last 'audio decoder: \d+ commands'
if ($a) { Write-Host (Get-Message $a) } else { Write-Host '(no audio decoder summary yet: it prints every 8,192 commands)' }
$lines | Where-Object { $_ -match 'audio decoder: (stream \d+ refused|bad packet|a packet)' } | Select-Object -First 6 | ForEach-Object { Write-Host (Get-Message $_) }
$voices = @($lines | Where-Object { $_ -match 'XMA voice disabled' })
if ($voices.Count) {
    $bad = @($voices | Where-Object { $_ -match 'bad_frames=([1-9]\d*)' }).Count
    $silent = @($voices | Where-Object { $_ -match 'peak=0\b' }).Count
    Write-Host "$($voices.Count) XMA voices ended: $bad with bad frames, $silent silent (peak 0)"
}

Write-Section 'GPU failures'
$fail = @($lines | Where-Object { $_ -match 'DEVICE_HUNG|DEVICE_REMOVED|host GPU failure|host GPU device removed|(0x887A000[0-9])' })
if ($fail.Count) {
    $fail | Select-Object -First 4 | ForEach-Object { $t = $_; if ($t.Length -gt 400) { $t = $t.Remove(400) + ' ...' }; Write-Host $t }
    Write-Host "($($fail.Count) lines; DXGI_ERROR_DEVICE_HUNG is the GPU timeout (TDR): also caused by unstable overclocks)"
} else { Write-Host '(none)' }

Write-Section 'D3D12 debug layer'
$d3d = @($lines | Where-Object { $_ -match 'D3D12: ' })
if ($d3d.Count) {
    $d3d | ForEach-Object { Get-Message $_ -Fold } | Group-Object | Sort-Object Count -Descending | Select-Object -First $Top |
        ForEach-Object { Write-Host ("{0,6} x {1}" -f $_.Count, $_.Name) }
} else { Write-Host '(none; run with -D3dDebug to collect)' }

Write-Section 'Shaders'
foreach ($p in '^warm ', 'refused', 'did not translate', 'without a host pipeline') {
    $n = @($lines | Where-Object { $_ -match $p }).Count
    if ($n) { Write-Host ("{0,6} lines matching '{1}'" -f $n, $p) }
}
$warm = Select-Last '^warm '
if ($warm) { Write-Host $warm }

Write-Section "Most frequent warnings and errors (top $Top)"
$lines | Where-Object { $_ -match '\s(WARN|ERROR)\s' } | ForEach-Object { Get-Message $_ -Fold } | Group-Object |
    Sort-Object Count -Descending | Select-Object -First $Top | ForEach-Object { Write-Host ("{0,6} x {1}" -f $_.Count, $_.Name) }

Write-Section 'Other known failure markers'
foreach ($p in 'dstorage: request failed', 'error 1455', 'unimplemented', 'trap') {
    $n = @($lines | Where-Object { $_ -match $p }).Count
    if ($n) { Write-Host "$n x '$p' - first: $($lines | Where-Object { $_ -match $p } | Select-Object -First 1)" }
}

Write-Section 'Crash reports'
$crashes = @(Get-ChildItem -LiteralPath $DataDir -Recurse -Filter crashes.log -File -ErrorAction SilentlyContinue | Where-Object { $_.LastWriteTime -ge $Since })
if ($crashes.Count) {
    foreach ($c in $crashes) {
        Write-Host "$($c.FullName) (written $($c.LastWriteTime))"
        Get-Content -LiteralPath $c.FullName -Tail 40 | ForEach-Object { Write-Host "  $_" }
    }
} else { Write-Host '(none written during this run)' }
