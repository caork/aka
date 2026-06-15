param(
  [Parameter(Mandatory = $true)]
  [string]$PortableZip,
  [int]$Seconds = 10
)

$ErrorActionPreference = "Stop"

if (!(Test-Path $PortableZip)) {
  throw "Portable zip not found: $PortableZip"
}

$stage = Join-Path ([System.IO.Path]::GetTempPath()) ("aka-desktop-smoke-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Expand-Archive -Path $PortableZip -DestinationPath $stage -Force

$exe = Join-Path $stage "AKA.exe"
if (!(Test-Path $exe)) {
  throw "AKA.exe missing from portable zip"
}

foreach ($entry in @(
  "README-Windows.txt",
  "resources\engine\codebase-memory-mcp.exe",
  "resources\client-integrations\clients\opencode\plugins\aka.js",
  "resources\client-integrations\clients\opencode\skills\aka-code-graph\SKILL.md"
)) {
  $path = Join-Path $stage $entry
  if (!(Test-Path $path)) {
    throw "Portable zip missing $entry"
  }
}

$webviewKeys = @(
  "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F1E7E5F8-4B5C-4B4E-8D6A-CB5F85F0E351}",
  "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F1E7E5F8-4B5C-4B4E-8D6A-CB5F85F0E351}",
  "HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F1E7E5F8-4B5C-4B4E-8D6A-CB5F85F0E351}"
)
$webviewInstalled = $false
foreach ($key in $webviewKeys) {
  if (Test-Path $key) {
    $webviewInstalled = $true
    break
  }
}
Write-Host "WebView2 runtime detected: $webviewInstalled"

$appdata = Join-Path $stage "appdata"
$userprofile = Join-Path $stage "userprofile"
New-Item -ItemType Directory -Force -Path $appdata, $userprofile | Out-Null

$env:APPDATA = $appdata
$env:USERPROFILE = $userprofile
$env:AKA_HOME = Join-Path $stage "aka-home"
$env:RUST_BACKTRACE = "1"

$stderr = Join-Path $stage "stderr.log"
$stdout = Join-Path $stage "stdout.log"
$process = Start-Process -FilePath $exe -WorkingDirectory $stage -PassThru -RedirectStandardError $stderr -RedirectStandardOutput $stdout

Start-Sleep -Seconds $Seconds

if ($process.HasExited) {
  Write-Host "AKA.exe exited early with code $($process.ExitCode)"
  if (Test-Path $stdout) {
    Write-Host "--- stdout ---"
    Get-Content $stdout -Raw
  }
  if (Test-Path $stderr) {
    Write-Host "--- stderr ---"
    Get-Content $stderr -Raw
  }
  Write-Host "--- appdata tree ---"
  Get-ChildItem -Path $appdata -Recurse -Force -ErrorAction SilentlyContinue | Select-Object FullName, Length | Format-Table -AutoSize
  throw "Windows desktop smoke failed: process exited before ${Seconds}s"
}

Stop-Process -Id $process.Id -Force
Wait-Process -Id $process.Id -ErrorAction SilentlyContinue

if (!(Test-Path (Join-Path $appdata "com.aka.desktop"))) {
  throw "Windows desktop smoke failed: app data directory was not created"
}

Write-Host "Windows desktop smoke passed: AKA.exe stayed alive for ${Seconds}s"
