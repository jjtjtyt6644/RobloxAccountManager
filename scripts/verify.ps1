# Run from the workspace root:  powershell -ExecutionPolicy Bypass -File scripts\verify.ps1
# 1. Proves tao/wry/WebView2 are NOT in the manager's dependency graph (only in ram-login).
# 2. Builds release and checks both exes exist side by side.
# 3. Starts the manager, leaves it idle for 5 minutes, reports working set / private bytes.
#    Don't touch the window or launch anything while it runs.
param([int]$IdleSeconds = 300, [double]$BudgetMB = 20)
$ErrorActionPreference = "Stop"
$target = "x86_64-pc-windows-msvc"

Write-Host "== 1. Dependency split ==" -ForegroundColor Cyan
$mgr = cargo tree -p roblox_account_manager -e normal --target $target --prefix none 2>&1
$bad = $mgr | Select-String -Pattern '^(tao|wry|webview2-com|webview2-com-sys) v'
if ($bad) { Write-Host "FAIL: manager pulls in webview crates:" -ForegroundColor Red; $bad; exit 1 }
Write-Host "OK  manager: no tao / wry / webview2-com"
$login = cargo tree -p ram-login -e normal --target $target --prefix none 2>&1 | Select-String -Pattern '^(tao|wry) v'
Write-Host "OK  ram-login has:" ($login -join ", ")

Write-Host "`n== 2. Release build ==" -ForegroundColor Cyan
cargo build --release --workspace
$exe = "target\release\roblox_account_manager.exe"
$helper = "target\release\ram-login.exe"
foreach ($f in @($exe, $helper)) { if (-not (Test-Path $f)) { Write-Host "FAIL: missing $f" -ForegroundColor Red; exit 1 } }
"{0,-40} {1,8:N0} KB" -f $exe, ((Get-Item $exe).Length / 1KB)
"{0,-40} {1,8:N0} KB" -f $helper, ((Get-Item $helper).Length / 1KB)
# Optional: with VS tools on PATH, confirm the manager doesn't import the WebView2 loader.
if (Get-Command dumpbin -ErrorAction SilentlyContinue) {
  $imports = dumpbin /dependents $exe | Select-String -Pattern 'WebView2'
  if ($imports) { Write-Host "FAIL: manager imports WebView2" -ForegroundColor Red; exit 1 } else { Write-Host "OK  manager imports no WebView2 DLL" }
}

Write-Host "`n== 3. Idle memory ($IdleSeconds s) ==" -ForegroundColor Cyan
$p = Start-Process -FilePath $exe -PassThru
Start-Sleep -Seconds $IdleSeconds
$p.Refresh()
$ws = $p.WorkingSet64 / 1MB
$priv = $p.PrivateMemorySize64 / 1MB
"Working set : {0:N1} MB" -f $ws
"Private     : {0:N1} MB" -f $priv
"Peak WS     : {0:N1} MB" -f ($p.PeakWorkingSet64 / 1MB)
if ($ws -le $BudgetMB) {
  Write-Host "UNDER budget ($BudgetMB MB): no trimming needed." -ForegroundColor Green
} else {
  Write-Host "OVER budget ($BudgetMB MB). Find the holder before trimming:" -ForegroundColor Yellow
  Write-Host "  - Sysinternals VMMap -> attach to roblox_account_manager.exe -> compare 'Image' vs 'Private Data' vs 'Heap'."
  Write-Host "  - Large 'Image' from an OpenGL driver DLL (nvoglv64 / atio6axx / ig*icd64) = the glow renderer's driver, not our code."
  Write-Host "  - Large 'Private Data'/'Heap' = ours: check font atlas size, tokio threads, SQLite cache."
}
Write-Host "Leaving the manager running so you can inspect it. Close it when done."
