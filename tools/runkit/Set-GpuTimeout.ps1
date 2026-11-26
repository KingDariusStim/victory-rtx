<#
.SYNOPSIS
  Raises the Windows GPU timeout (TDR) so that multi-second GPU frames are not
  treated as a hung GPU, or restores the default. Needs an elevated prompt and
  a reboot to take effect.

  Windows resets the display driver when one GPU submission runs longer than
  TdrDelay seconds (default 2). The runtime can submit work that long at its
  current 1-4 fps, which ends the run with DXGI_ERROR_DEVICE_HUNG/REMOVED even
  when nothing is actually hung. A real hang then takes longer to recover from
  (up to the new delay) before the screen comes back.

.EXAMPLE
  .\Set-GpuTimeout.ps1              # TdrDelay 10 s, TdrDdiDelay 20 s
  .\Set-GpuTimeout.ps1 -Seconds 30
  .\Set-GpuTimeout.ps1 -Undo        # back to the Windows defaults
#>
[CmdletBinding()]
param([int] $Seconds = 10, [switch] $Undo)

$ErrorActionPreference = 'Stop'
$key = 'HKLM:\SYSTEM\CurrentControlSet\Control\GraphicsDrivers'
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (!$admin) { throw 'Run this from an elevated PowerShell (Run as administrator).' }

if ($Undo) {
    Remove-ItemProperty -Path $key -Name TdrDelay, TdrDdiDelay -ErrorAction SilentlyContinue
    Write-Host 'GPU timeout back to the Windows defaults. Reboot to apply.'
} else {
    if ($Seconds -lt 2 -or $Seconds -gt 60) { throw 'Use 2 to 60 seconds.' }
    New-ItemProperty -Path $key -Name TdrDelay -PropertyType DWord -Value $Seconds -Force | Out-Null
    New-ItemProperty -Path $key -Name TdrDdiDelay -PropertyType DWord -Value ($Seconds * 2) -Force | Out-Null
    Write-Host "GPU timeout set to $Seconds s (driver calls $($Seconds * 2) s). Reboot to apply."
}
Get-ItemProperty -Path $key | Select-Object TdrDelay, TdrDdiDelay | Format-List
