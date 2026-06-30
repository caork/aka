<# aka client integration installer for Windows.

Usage:
  .\clients\install.ps1 -Check
  .\clients\install.ps1 -Client codex [-DryRun] [-Reinstall] [-McpUrl http://127.0.0.1:4112/mcp]
  .\clients\install.ps1 -Client claude-code [-DryRun] [-Reinstall] [-Plugin]
  .\clients\install.ps1 -Client opencode [-DryRun] [-Reinstall]

Default mode configures clients to connect to the running AKA desktop MCP endpoint:
http://127.0.0.1:4112/mcp
#>

[CmdletBinding()]
param(
  [string]$Client = "",

  [string]$McpUrl = $(if ($env:AKA_MCP_URL) { $env:AKA_MCP_URL } else { "http://127.0.0.1:4112/mcp" }),
  [string]$Bin = "",
  [switch]$Plugin,
  [switch]$Stdio,
  [switch]$Check,
  [switch]$DryRun,
  [switch]$Reinstall
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ValidClients = @("claude-code", "codex", "opencode")

function Info($message) {
  Write-Host "[aka] $message" -ForegroundColor Cyan
}

function Warn($message) {
  Write-Warning "[aka] $message"
}

function Run-Step($description, [scriptblock]$Action) {
  if ($DryRun) {
    Info "[dry-run] $description"
    return
  }
  & $Action
}

function Test-CommandExists($name) {
  $null -ne (Get-Command $name -ErrorAction SilentlyContinue)
}

function Test-AkaMcpEndpoint {
  try {
    $response = Invoke-WebRequest `
      -UseBasicParsing `
      -Method Post `
      -Uri $McpUrl `
      -TimeoutSec 2 `
      -ContentType "application/json" `
      -Headers @{ Accept = "application/json, text/event-stream" } `
      -Body '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"aka-installer","version":"0"}}}'
    return $response.StatusCode -eq 200 -and $response.Content -match '"name":"aka-mcp"'
  } catch {
    return $false
  }
}

function Write-CheckResult($label, $ok, $detail) {
  $status = if ($ok) { "ok" } else { "missing" }
  $color = if ($ok) { "Green" } else { "Yellow" }
  Write-Host ("{0,-18} {1,-8} {2}" -f $label, $status, $detail) -ForegroundColor $color
}

function Get-AkaBin {
  if ($Bin) {
    if (!(Test-Path $Bin)) { throw "-Bin not found: $Bin" }
    return (Resolve-Path $Bin).Path
  }
  foreach ($name in @("AKA.exe", "aka.exe", "AKA", "aka")) {
    $cmd = Get-Command $name -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
  }
  foreach ($candidate in @(
    (Join-Path $ScriptDir "..\target\release\aka.exe"),
    (Join-Path $ScriptDir "..\target\debug\aka.exe"),
    (Join-Path $ScriptDir "..\apps\desktop\src-tauri\target\release\AKA.exe")
  )) {
    if (Test-Path $candidate) { return (Resolve-Path $candidate).Path }
  }
  throw "Cannot find AKA executable. Pass -Bin C:\path\to\AKA.exe for stdio fallback."
}

function Get-ClientStatus {
  $homeDir = [Environment]::GetFolderPath("UserProfile")
  $codexConfig = Join-Path $homeDir ".codex\config.toml"
  $opencodeConfig = Join-Path $homeDir ".config\opencode\opencode.json"
  $opencodePlugin = Join-Path $homeDir ".config\opencode\plugins\aka.js"
  $opencodeSkill = Join-Path $homeDir ".config\opencode\skills\aka-code-graph\SKILL.md"

  Write-CheckResult "AKA MCP" (Test-AkaMcpEndpoint) $McpUrl
  if (!$Client -or $Client -eq "claude-code") {
    Write-CheckResult "Claude CLI" (Test-CommandExists "claude") "claude"
  }
  if (!$Client -or $Client -eq "codex") {
    Write-CheckResult "Codex CLI" (Test-CommandExists "codex") "codex"
    Write-CheckResult "Codex config" ((Test-Path $codexConfig) -and ((Get-Content $codexConfig -Raw) -match "\[mcp_servers\.aka\]")) $codexConfig
  }
  if (!$Client -or $Client -eq "opencode") {
    Write-CheckResult "OpenCode config" ((Test-Path $opencodeConfig) -and ((Get-Content $opencodeConfig -Raw) -match '"aka"')) $opencodeConfig
    Write-CheckResult "OpenCode plugin" (Test-Path $opencodePlugin) $opencodePlugin
    Write-CheckResult "OpenCode skill" (Test-Path $opencodeSkill) $opencodeSkill
  }
}

function Remove-CodexAkaBlock($cfg) {
  if (!(Test-Path $cfg)) { return }
  $backup = "$cfg.aka-backup-$(Get-Date -Format yyyyMMddHHmmss)"
  Run-Step "backup $cfg to $backup and remove existing [mcp_servers.aka]" {
    Copy-Item $cfg $backup -Force
    $lines = Get-Content $cfg
    $out = New-Object System.Collections.Generic.List[string]
    $skip = $false
    foreach ($line in $lines) {
      if ($line -match '^\[mcp_servers\.aka\]$') {
        $skip = $true
        continue
      }
      if ($skip -and $line -match '^\[') {
        $skip = $false
      }
      if (!$skip) {
        $out.Add($line)
      }
    }
    Set-Content -Encoding UTF8 $cfg -Value $out
  }
}

function Install-Codex {
  $homeDir = [Environment]::GetFolderPath("UserProfile")
  $cfg = Join-Path $homeDir ".codex\config.toml"
  $hasCodexCli = Test-CommandExists "codex"
  if ((Test-Path $cfg) -and ((Get-Content $cfg -Raw) -match "\[mcp_servers\.aka\]") -and -not $Reinstall) {
    Info "$cfg already has [mcp_servers.aka]. Use -Reinstall to rewrite."
    return
  }
  if ($Reinstall -and (Test-Path $cfg)) {
    Remove-CodexAkaBlock $cfg
  }
  if ($hasCodexCli -and -not $Stdio) {
    Run-Step "codex mcp add aka --url $McpUrl" { codex mcp add aka --url $McpUrl }
    return
  }
  if ($hasCodexCli -and $Stdio) {
    $akaBin = Get-AkaBin
    Run-Step "codex mcp add aka -- $akaBin mcp" { codex mcp add aka -- $akaBin mcp }
    return
  }

  Run-Step "write $cfg" {
    New-Item -ItemType Directory -Force (Split-Path -Parent $cfg) | Out-Null
    if (!(Test-Path $cfg)) {
      "" | Set-Content -Encoding UTF8 $cfg
    }
    Add-Content -Encoding UTF8 $cfg ""
    Add-Content -Encoding UTF8 $cfg "[mcp_servers.aka]"
    if ($Stdio) {
      $akaBin = (Get-AkaBin).Replace("\", "\\")
      Add-Content -Encoding UTF8 $cfg "command = `"$akaBin`""
      Add-Content -Encoding UTF8 $cfg 'args = ["mcp"]'
    } else {
      Add-Content -Encoding UTF8 $cfg "url = `"$McpUrl`""
    }
  }
}

function Install-ClaudeCode {
  if (!(Test-CommandExists "claude")) {
    throw "Claude Code CLI not found. Install Claude Code first, then rerun this script."
  }
  if ($Plugin) {
    if (!$Reinstall) {
      $plugins = try { (claude plugin list 2>$null | Out-String) } catch { "" }
      if ($plugins -match "aka@aka") {
        Info "Plugin aka@aka already installed. Use -Reinstall to rewrite."
        return
      }
    }
    if ($Reinstall) {
      Run-Step "claude plugin uninstall aka@aka" {
        try { claude plugin uninstall aka@aka } catch { }
      }
    }
    $marketplaceDir = if (Test-Path (Join-Path $ScriptDir ".claude-plugin\marketplace.json")) { $ScriptDir } else { Split-Path -Parent $ScriptDir }
    Run-Step "claude plugin marketplace add $marketplaceDir" { claude plugin marketplace add $marketplaceDir }
    Run-Step "claude plugin install aka@aka" { claude plugin install aka@aka }
    return
  }
  if (!$Reinstall) {
    $servers = try { (claude mcp list 2>$null | Out-String) } catch { "" }
    if ($servers -match "(?m)^aka([: ]|$)") {
      Info "MCP server 'aka' already exists. Use -Reinstall to rewrite."
      return
    }
  }
  if ($Stdio) {
    $akaBin = Get-AkaBin
    if ($Reinstall) {
      Run-Step "claude mcp remove aka" {
        try { claude mcp remove aka } catch { }
      }
    }
    Run-Step "claude mcp add --transport stdio --scope user aka -- $akaBin mcp" {
      claude mcp add --transport stdio --scope user aka -- $akaBin mcp
    }
  } else {
    if ($Reinstall) {
      Run-Step "claude mcp remove aka" {
        try { claude mcp remove aka } catch { }
      }
    }
    Run-Step "claude mcp add --transport http --scope user aka $McpUrl" {
      claude mcp add --transport http --scope user aka $McpUrl
    }
  }
}

function Install-OpenCode {
  $homeDir = [Environment]::GetFolderPath("UserProfile")
  $cfgDir = Join-Path $homeDir ".config\opencode"
  $cfg = Join-Path $cfgDir "opencode.json"
  $pluginSrc = Join-Path $ScriptDir "opencode\plugins\aka.js"
  $skillSrc = Join-Path $ScriptDir "opencode\skills\aka-code-graph"
  $pluginDst = Join-Path $cfgDir "plugins\aka.js"
  $skillDst = Join-Path $cfgDir "skills\aka-code-graph"

  Run-Step "configure OpenCode mcp.aka in $cfg" {
    New-Item -ItemType Directory -Force $cfgDir | Out-Null
    if (!(Test-Path $cfg)) {
      '{ "$schema": "https://opencode.ai/config.json" }' | Set-Content -Encoding UTF8 $cfg
    }
    $json = Get-Content $cfg -Raw | ConvertFrom-Json
    if ($null -eq $json.mcp) {
      $json | Add-Member -NotePropertyName mcp -NotePropertyValue ([pscustomobject]@{})
    }
    if ($json.mcp.PSObject.Properties.Name -contains "aka") {
      $json.mcp.aka = [pscustomobject]@{ type = "remote"; url = $McpUrl; enabled = $true }
    } else {
      $json.mcp | Add-Member -NotePropertyName aka -NotePropertyValue ([pscustomobject]@{ type = "remote"; url = $McpUrl; enabled = $true })
    }
    $json | ConvertTo-Json -Depth 20 | Set-Content -Encoding UTF8 $cfg
  }

  if (Test-Path $pluginSrc) {
    Run-Step "copy OpenCode plugin to $pluginDst" {
      New-Item -ItemType Directory -Force (Split-Path -Parent $pluginDst) | Out-Null
      Copy-Item $pluginSrc $pluginDst -Force
    }
  } else {
    Warn "OpenCode plugin source not found: $pluginSrc"
  }
  if (Test-Path $skillSrc) {
    Run-Step "copy OpenCode skill to $skillDst" {
      New-Item -ItemType Directory -Force (Split-Path -Parent $skillDst) | Out-Null
      if ($Reinstall -and (Test-Path $skillDst)) {
        Remove-Item $skillDst -Recurse -Force
      }
      Copy-Item $skillSrc (Split-Path -Parent $skillDst) -Recurse -Force
    }
  } else {
    Warn "OpenCode skill source not found: $skillSrc"
  }
}

if ($Check) {
  if ($Client -and $Client -notin $ValidClients) {
    throw "Unsupported -Client: $Client. Use claude-code, codex, or opencode."
  }
  Get-ClientStatus
  exit 0
}

if (!$Client) {
  throw "-Client is required unless -Check is used."
}
if ($Client -notin $ValidClients) {
  throw "Unsupported -Client: $Client. Use claude-code, codex, or opencode."
}

switch ($Client) {
  "codex" { Install-Codex }
  "claude-code" { Install-ClaudeCode }
  "opencode" { Install-OpenCode }
}

Info "Done. Start AKA desktop, then verify with the client MCP list command or ask the agent to list aka repositories."
