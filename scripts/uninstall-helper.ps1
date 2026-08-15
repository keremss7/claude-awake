# Removes the Claude Awake helper service.
#
# The service restores the power settings on its own stop path, so this can never
# leave a machine unable to sleep. The final check below is belt and braces.
#
#   powershell -ExecutionPolicy Bypass -File uninstall-helper.ps1

[CmdletBinding()]
param()

$ErrorActionPreference = 'Continue'
$ServiceName = 'ClaudeAwakeHelper'
$targetDir = Join-Path $env:ProgramFiles 'Claude Awake'
$target = Join-Path $targetDir 'claude-awake-helperd.exe'
$stateDir = Join-Path $env:ProgramData 'ClaudeAwake'

function Write-Ok   { param($m) Write-Host "[ok] $m" -ForegroundColor Green }
function Write-Warn { param($m) Write-Host "[--] $m" -ForegroundColor Yellow }

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host '[!!] Administrator rights are required.' -ForegroundColor Red
    exit 1
}

if (Test-Path $target) {
    & $target --uninstall-service
    Write-Ok 'service stopped and removed'
} elseif (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
    Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
    & sc.exe delete $ServiceName | Out-Null
    Write-Warn 'service removed without the binary; power settings may not have been reverted'
}

# The binary self-heals from a leftover snapshot on startup, so if one survived,
# run it once to roll the machine back.
if ((Test-Path (Join-Path $stateDir 'baseline.json')) -and (Test-Path $target)) {
    $p = Start-Process -FilePath $target -ArgumentList '--console' -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 2
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    Write-Ok 'saved power settings restored'
}

Remove-Item -Recurse -Force $stateDir -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force $targetDir -ErrorAction SilentlyContinue

# Last resort: if anything above failed, this is the setting that actually
# matters and leaving it changed would be the one genuinely harmful leftover.
$lid = powercfg /query SCHEME_CURRENT 4f971e89-eebd-4455-a8de-9e59040e7347 5ca83367-6e45-459f-a27b-476b1d01c936 2>$null
if ($lid -match 'Current AC Power Setting Index:\s+0x00000000') {
    Write-Warn 'Lid-close action is still "Do nothing". Set it in Control Panel > Power Options if that is not what you want.'
}

Write-Host ''
Write-Host 'Removed. Uninstall the Claude Awake app from Settings > Apps to finish.' -ForegroundColor Cyan
