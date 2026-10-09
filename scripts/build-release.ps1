# Builds the single-file release:  powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1
# ram-login.exe is built first, then embedded into roblox_account_manager.exe (see manager\build.rs),
# so dist\roblox_account_manager.exe is the ONLY file users need.
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

$version = (Select-String -Path manager\Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
Write-Host "== Building v$version ==" -ForegroundColor Cyan

cargo build --release -p ram-login
if ($LASTEXITCODE -ne 0) { exit 1 }
cargo build --release -p roblox_account_manager
if ($LASTEXITCODE -ne 0) { exit 1 }

$login = Get-Item target\release\ram-login.exe
$mgr = Get-Item target\release\roblox_account_manager.exe
# The manager must be at least the helper's size bigger than it would be without it; a cheap sanity check
# is that the helper's bytes appear inside it.
$needle = [System.IO.File]::ReadAllBytes($login.FullName)[1024..1087]
$hay = [System.IO.File]::ReadAllBytes($mgr.FullName)
$found = $false
for ($i = 0; $i -le $hay.Length - $needle.Length -and -not $found; $i += 1) {
  if ($hay[$i] -eq $needle[0]) {
    $ok = $true
    for ($j = 1; $j -lt $needle.Length; $j++) { if ($hay[$i + $j] -ne $needle[$j]) { $ok = $false; break } }
    if ($ok) { $found = $true }
  }
}
if (-not $found) { Write-Host "FAIL: ram-login.exe is not embedded in the manager" -ForegroundColor Red; exit 1 }
Write-Host "OK  sign-in helper embedded"

New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item $mgr.FullName dist\roblox_account_manager.exe -Force
"{0,-40} {1,8:N0} KB" -f "dist\roblox_account_manager.exe", ((Get-Item dist\roblox_account_manager.exe).Length / 1KB)
Write-Host ""
Write-Host "Publish: create a GitHub release tagged v$version and attach dist\roblox_account_manager.exe" -ForegroundColor Green
Write-Host "  (the file name must stay roblox_account_manager.exe - the in-app updater looks for it)"
