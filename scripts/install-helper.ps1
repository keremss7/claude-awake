# Installs the privileged half of Claude Awake as a Windows service.
#
# This is the one-time administrator step. Afterwards the app toggles protection
# over a named pipe and never asks for elevation again.
#
#   Right-click > Run with PowerShell, or from an elevated prompt:
#     powershell -ExecutionPolicy Bypass -File install-helper.ps1
#
# The NSIS installer runs the same command automatically, so this script is only
# needed for portable or development installs.

[CmdletBinding()]
param(
    # Explicit path to claude-awake-helperd.exe. Discovered automatically if omitted.
    [string]$HelperPath
)

$ErrorActionPreference = 'Stop'
$ServiceName = 'ClaudeAwakeHelper'
$PipeName = 'claude-awake'

function Write-Ok    { param($m) Write-Host "[ok] $m" -ForegroundColor Green }
function Write-Fail  { param($m) Write-Host "[!!] $m" -ForegroundColor Red; exit 1 }

# --- elevation ---------------------------------------------------------------
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Fail 'Administrator rights are required. Re-run this from an elevated PowerShell prompt.'
}

# --- locate the helper -------------------------------------------------------
if (-not $HelperPath) {
    $here = Split-Path -Parent $MyInvocation.MyCommand.Path
    $candidates = @(
        (Join-Path $here 'claude-awake-helperd.exe')
        (Join-Path $here '..\src-tauri\resources\claude-awake-helperd.exe')
        (Join-Path $here '..\src-tauri\target\release\claude-awake-helperd.exe')
        (Join-Path $here '..\src-tauri\target\debug\claude-awake-helperd.exe')
    )
    $HelperPath = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $HelperPath -or -not (Test-Path $HelperPath)) {
    Write-Fail 'claude-awake-helperd.exe not found. Build it first: npm run helper:build'
}
$HelperPath = (Resolve-Path $HelperPath).Path
Write-Ok "helper found: $HelperPath"

# --- install to a stable location -------------------------------------------
# A service must not point at a build directory that could be deleted or, worse,
# be writable by a non-administrator.
$targetDir = Join-Path $env:ProgramFiles 'Claude Awake'
$target = Join-Path $targetDir 'claude-awake-helperd.exe'

if ($HelperPath -ne $target) {
    New-Item -ItemType Directory -Force -Path $targetDir | Out-Null
    if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
        # The file is locked while the old service runs.
        Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 500
    }
    Copy-Item -Path $HelperPath -Destination $target -Force
    Write-Ok "installed: $target"
}

# --- register and start ------------------------------------------------------
# The binary registers itself, which sidesteps sc.exe's binPath quoting rules.
& $target --install-service
if ($LASTEXITCODE -ne 0) { Write-Fail 'Service registration failed.' }

# --- verify ------------------------------------------------------------------
$deadline = (Get-Date).AddSeconds(10)
$ready = $false
while ((Get-Date) -lt $deadline) {
    if (Test-Path "\\.\pipe\$PipeName") { $ready = $true; break }
    Start-Sleep -Milliseconds 250
}
if (-not $ready) {
    Write-Fail "Service started but the pipe never appeared. Check: Get-EventLog -LogName Application -Source $ServiceName"
}

Write-Ok 'service running, pipe ready'
Write-Host ''
Write-Host 'Claude Awake can now keep this machine running with the lid closed.' -ForegroundColor Cyan
Write-Host "To remove it: uninstall-helper.ps1"
