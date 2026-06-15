param(
  [Parameter(Mandatory = $true)]
  [string]$PortableZip,
  [string]$SetupExe = "",
  [int]$Seconds = 10
)

$ErrorActionPreference = "Stop"

function Test-WebView2Runtime {
  $clientRoots = @(
    "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients",
    "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients",
    "HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients"
  )
  foreach ($root in $clientRoots) {
    if (!(Test-Path $root)) {
      continue
    }
    foreach ($client in Get-ChildItem -Path $root -ErrorAction SilentlyContinue) {
      $props = Get-ItemProperty -Path $client.PSPath -ErrorAction SilentlyContinue
      if ($props.name -match "WebView2" -and $props.pv) {
        Write-Host "WebView2 registry entry: $($props.name) $($props.pv)"
        return $true
      }
    }
  }

  $programRoots = @(
    ${env:ProgramFiles(x86)},
    $env:ProgramFiles,
    $env:LOCALAPPDATA
  ) | Where-Object { $_ }
  foreach ($root in $programRoots) {
    $edgeWebView = Join-Path $root "Microsoft\EdgeWebView\Application"
    if (Test-Path $edgeWebView) {
      $exe = Get-ChildItem -Path $edgeWebView -Recurse -Filter "msedgewebview2.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
      if ($exe) {
        Write-Host "WebView2 executable: $($exe.FullName)"
        return $true
      }
    }
  }

  return $false
}

function Show-SmokeTree {
  param([string]$Path)
  if (Test-Path $Path) {
    Get-ChildItem -Path $Path -Recurse -Force -ErrorAction SilentlyContinue | Select-Object FullName, Length | Format-Table -AutoSize
  } else {
    Write-Host "Path does not exist: $Path"
  }
}

if (!(Test-Path $PortableZip)) {
  throw "Portable zip not found: $PortableZip"
}
if ($SetupExe -and !(Test-Path $SetupExe)) {
  throw "Setup exe not found: $SetupExe"
}

if (!(Test-WebView2Runtime)) {
  Write-Host "WebView2 runtime detected: False"
  if (!$SetupExe) {
    throw "WebView2 runtime missing and no setup.exe was provided"
  }
  Write-Host "Installing AKA setup silently to exercise the bundled WebView2 bootstrapper"
  $setupLogPrefix = Join-Path ([System.IO.Path]::GetTempPath()) ("aka-setup-" + [System.Guid]::NewGuid().ToString("N"))
  $setupStdout = "$setupLogPrefix.stdout.log"
  $setupStderr = "$setupLogPrefix.stderr.log"
  $setup = Start-Process -FilePath $SetupExe -ArgumentList "/S" -PassThru -Wait -RedirectStandardOutput $setupStdout -RedirectStandardError $setupStderr
  if ($setup.ExitCode -ne 0) {
    if (Test-Path $setupStdout) {
      Write-Host "--- setup stdout ---"
      Get-Content $setupStdout -Raw
    }
    if (Test-Path $setupStderr) {
      Write-Host "--- setup stderr ---"
      Get-Content $setupStderr -Raw
    }
    throw "AKA setup failed with code $($setup.ExitCode)"
  }
  if (!(Test-WebView2Runtime)) {
    throw "AKA setup completed but WebView2 runtime is still missing"
  }
} else {
  Write-Host "WebView2 runtime detected: True"
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

$appdata = Join-Path $stage "appdata"
$localappdata = Join-Path $stage "localappdata"
$userprofile = Join-Path $stage "userprofile"
New-Item -ItemType Directory -Force -Path $appdata, $localappdata, $userprofile | Out-Null

$env:APPDATA = $appdata
$env:LOCALAPPDATA = $localappdata
$env:USERPROFILE = $userprofile
$env:AKA_HOME = Join-Path $stage "aka-home"
$env:RUST_BACKTRACE = "1"

$stderr = Join-Path $stage "stderr.log"
$stdout = Join-Path $stage "stdout.log"
$process = Start-Process -FilePath $exe -WorkingDirectory $stage -PassThru -RedirectStandardError $stderr -RedirectStandardOutput $stdout

$windowHandle = [IntPtr]::Zero
for ($i = 0; $i -lt ($Seconds * 2); $i++) {
  Start-Sleep -Milliseconds 500
  $process.Refresh()
  if ($process.HasExited) {
    break
  }
  if ($process.MainWindowHandle -ne [IntPtr]::Zero) {
    $windowHandle = $process.MainWindowHandle
    break
  }
}

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
  Show-SmokeTree $appdata
  Write-Host "--- localappdata tree ---"
  Show-SmokeTree $localappdata
  throw "Windows desktop smoke failed: process exited before ${Seconds}s"
}

if ($windowHandle -eq [IntPtr]::Zero) {
  if (Test-Path $stdout) {
    Write-Host "--- stdout ---"
    Get-Content $stdout -Raw
  }
  if (Test-Path $stderr) {
    Write-Host "--- stderr ---"
    Get-Content $stderr -Raw
  }
  Write-Host "--- appdata tree ---"
  Show-SmokeTree $appdata
  Write-Host "--- localappdata tree ---"
  Show-SmokeTree $localappdata
  Stop-Process -Id $process.Id -Force
  Wait-Process -Id $process.Id -ErrorAction SilentlyContinue
  throw "Windows desktop smoke failed: main window handle was not created before ${Seconds}s"
}

Stop-Process -Id $process.Id -Force
Wait-Process -Id $process.Id -ErrorAction SilentlyContinue

if (!(Get-ChildItem -Path $appdata, $localappdata -Recurse -Directory -Filter "aka-home" -ErrorAction SilentlyContinue | Select-Object -First 1)) {
  Write-Host "warning: aka-home directory was not found in isolated app data roots"
  Write-Host "--- appdata tree ---"
  Show-SmokeTree $appdata
  Write-Host "--- localappdata tree ---"
  Show-SmokeTree $localappdata
}

Write-Host "Windows desktop smoke passed: AKA.exe created a main window handle $windowHandle and stayed alive"
