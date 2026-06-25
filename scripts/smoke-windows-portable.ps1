param(
    [Parameter(Mandatory = $true)]
    [string]$PortableDir,

    [string]$WorkDir = "$env:TEMP\aka-windows-smoke"
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$portable = (Resolve-Path $PortableDir).Path
$akaExe = Join-Path $portable "AKA.exe"
if (!(Test-Path $akaExe)) {
    throw "Missing AKA.exe in $portable"
}
Write-Host "aka smoke: portable=$portable"

Remove-Item -Recurse -Force $WorkDir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $WorkDir | Out-Null
$repo = Join-Path $WorkDir "spring-demo"
$akaHome = Join-Path $WorkDir "aka-home"
New-Item -ItemType Directory -Force $repo, (Join-Path $repo "src\main\java\com\example\demo") | Out-Null

@"
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>com.example</groupId>
  <artifactId>spring-demo</artifactId>
  <version>0.0.1</version>
  <dependencies>
    <dependency>
      <groupId>org.springframework.boot</groupId>
      <artifactId>spring-boot-starter-web</artifactId>
      <version>3.3.1</version>
    </dependency>
  </dependencies>
</project>
"@ | Set-Content -Encoding UTF8 (Join-Path $repo "pom.xml")

@"
package com.example.demo;

import org.springframework.boot.ApplicationArguments;
import org.springframework.boot.ApplicationRunner;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class OrderController implements ApplicationRunner {
    private final OrderService service = new OrderService();

    @GetMapping("/orders/{id}")
    public String getOrder(@PathVariable String id) {
        return service.loadOrder(id);
    }

    @Override
    public void run(ApplicationArguments args) {
        service.reindexOrders();
    }
}
"@ | Set-Content -Encoding UTF8 (Join-Path $repo "src\main\java\com\example\demo\OrderController.java")

@"
package com.example.demo;

public class OrderService {
    public String loadOrder(String id) { return "order-" + id; }
    public void reindexOrders() { loadOrder("boot"); }
}
"@ | Set-Content -Encoding UTF8 (Join-Path $repo "src\main\java\com\example\demo\OrderService.java")

function Wait-TcpPort {
    param(
        [string]$HostName,
        [int]$Port,
        [int]$TimeoutSeconds = 30
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if (Test-NetConnection -ComputerName $HostName -Port $Port -InformationLevel Quiet) {
            return
        }
        Start-Sleep -Milliseconds 500
    }
    throw "$HostName`:$Port did not open within $TimeoutSeconds seconds"
}

$script:McpSessionId = $null

function ConvertFrom-McpResponse {
    param([string[]]$Lines)
    foreach ($line in $Lines) {
        $trimmed = $line.Trim()
        if ($trimmed.StartsWith("{")) {
            return $trimmed | ConvertFrom-Json
        }
        if (!$trimmed.StartsWith("data:")) {
            continue
        }
        $payload = $trimmed.Substring(5).Trim()
        if ($payload.StartsWith("{")) {
            return $payload | ConvertFrom-Json
        }
    }
    throw "MCP response did not contain a JSON payload"
}

function Invoke-McpRaw {
    param(
        [hashtable]$Body,
        [switch]$Notification,
        [int]$TimeoutSeconds = 120
    )
    $json = $Body | ConvertTo-Json -Depth 16 -Compress
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
    $request = [System.Net.HttpWebRequest]::Create("http://127.0.0.1:4112/mcp")
    $request.Method = "POST"
    $request.ContentType = "application/json"
    $request.Accept = "application/json, text/event-stream"
    $request.Timeout = $TimeoutSeconds * 1000
    $request.ReadWriteTimeout = $TimeoutSeconds * 1000
    $request.ContentLength = $bytes.Length
    if ($script:McpSessionId) {
        $request.Headers.Add("Mcp-Session-Id", $script:McpSessionId)
    }
    $requestStream = $request.GetRequestStream()
    try {
        $requestStream.Write($bytes, 0, $bytes.Length)
    } finally {
        $requestStream.Dispose()
    }
    if ($Notification) {
        $response = $request.GetResponse()
        $response.Dispose()
        return $null
    }
    $response = $request.GetResponse()
    $session = $response.Headers["Mcp-Session-Id"]
    if ($session) {
        $script:McpSessionId = [string]$session
    }
    $reader = [System.IO.StreamReader]::new($response.GetResponseStream(), [System.Text.Encoding]::UTF8)
    try {
        $lines = New-Object System.Collections.Generic.List[string]
        while (($line = $reader.ReadLine()) -ne $null) {
            $lines.Add($line)
            $trimmed = $line.Trim()
            if ($trimmed.StartsWith("{") -or ($trimmed.StartsWith("data:") -and $trimmed.Substring(5).Trim().StartsWith("{"))) {
                return ConvertFrom-McpResponse -Lines $lines
            }
        }
        ConvertFrom-McpResponse -Lines $lines
    } finally {
        $reader.Dispose()
        $response.Dispose()
    }
}

function Invoke-McpTool {
    param(
        [int]$Id,
        [string]$Name,
        [hashtable]$Arguments
    )
    Invoke-McpRaw -Body @{
        jsonrpc = "2.0"
        id = $Id
        method = "tools/call"
        params = @{
            name = $Name
            arguments = $Arguments
        }
    }
}

function Get-ToolTextJson {
    param($ToolResult)
    if ($ToolResult.result.isError) {
        throw "MCP tool failed: $(($ToolResult.result.content | ForEach-Object { $_.text }) -join "`n")"
    }
    $text = ($ToolResult.result.content | ForEach-Object { $_.text }) -join "`n"
    if (!$text.Trim().StartsWith("{") -and !$text.Trim().StartsWith("[")) {
        return $text
    }
    $text | ConvertFrom-Json
}

function Get-DesktopLogTail {
    $logPath = Join-Path $env:APPDATA "com.aka.desktop\logs\aka-desktop.log"
    if (Test-Path $logPath) {
        return Get-Content $logPath -Tail 160 | Out-String
    }
    return ""
}

function Wait-McpRepoReady {
    param(
        [string]$ExpectedRepo,
        [int]$TimeoutSeconds = 180
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $lastText = ""
    while ((Get-Date) -lt $deadline) {
        $reposResult = Invoke-McpTool -Id 30 -Name "list_repos" -Arguments @{}
        if (!$reposResult.result.isError) {
            $text = ($reposResult.result.content | ForEach-Object { $_.text }) -join "`n"
            $lastText = $text
            $repos = $text | ConvertFrom-Json
            foreach ($repoInfo in $repos.repos) {
                if ($repoInfo.name -eq $ExpectedRepo -and $repoInfo.status -eq "ready") {
                    return $repoInfo
                }
                if ($repoInfo.name -eq $ExpectedRepo -and $repoInfo.status -eq "failed") {
                    throw "MCP repo $ExpectedRepo failed:`n$text`nDesktop log:`n$(Get-DesktopLogTail)"
                }
            }
        }
        Start-Sleep -Seconds 1
    }
    throw "MCP list_repos did not show a ready $ExpectedRepo repo within $TimeoutSeconds seconds`nLast list_repos:`n$lastText`nDesktop log:`n$(Get-DesktopLogTail)"
}

Get-Process AKA -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
$desktopLogPath = Join-Path $env:APPDATA "com.aka.desktop\logs\aka-desktop.log"
Remove-Item -Force $desktopLogPath -ErrorAction SilentlyContinue
$env:AKA_HOME = $akaHome
$env:AKA_ENGINE_EMBEDDED = "require"
Write-Host "aka smoke: starting AKA.exe"
$desktop = Start-Process -FilePath $akaExe -PassThru
try {
    Wait-TcpPort -HostName "127.0.0.1" -Port 4112 -TimeoutSeconds 45
    Write-Host "aka smoke: MCP port ready"

    Write-Host "aka smoke: MCP initialize"
    $init = Invoke-McpRaw -Body @{
        jsonrpc = "2.0"
        id = 1
        method = "initialize"
        params = @{
            protocolVersion = "2025-06-18"
            capabilities = @{}
            clientInfo = @{
                name = "aka-windows-portable-smoke"
                version = "1.0"
            }
        }
    }
    Write-Host "aka smoke: MCP tools/list"
    $tools = Invoke-McpRaw -Body @{ jsonrpc = "2.0"; id = 2; method = "tools/list"; params = @{} }
    foreach ($toolName in @("analyze", "list_repos", "search_code", "query", "context")) {
        if (!($tools.result.tools | Where-Object { $_.name -eq $toolName })) {
            throw "MCP tools/list missing $toolName"
        }
    }

    Write-Host "aka smoke: MCP analyze"
    $analyze = Invoke-McpTool -Id 3 -Name "analyze" -Arguments @{ repo_path = $repo }
    $analyzeOut = Get-ToolTextJson -ToolResult $analyze
    $repoName = if ($analyzeOut.repo) { [string]$analyzeOut.repo } else { "spring-demo" }
    Write-Host "aka smoke: analyze repo=$repoName status=$($analyzeOut.status)"
    Write-Host "aka smoke: waiting MCP repo readiness"
    $readyRepo = Wait-McpRepoReady -ExpectedRepo $repoName
    $progressText = ""
    if ($readyRepo.progress -and $readyRepo.progress.logs) {
        $progressText = ($readyRepo.progress.logs | Out-String)
    }
    if ($progressText -notmatch "aka-engine:index:embedded") {
        throw "repo progress did not show actual embedded engine indexing`nProgress:`n$progressText`nDesktop log:`n$(Get-DesktopLogTail)"
    }
    if ($progressText -match "falling back to binary engine") {
        throw "repo progress shows embedded engine fallback despite require mode`nProgress:`n$progressText"
    }

    Write-Host "aka smoke: MCP list/search/context"
    $listRepos = Invoke-McpTool -Id 4 -Name "list_repos" -Arguments @{}
    $searchCode = Invoke-McpTool -Id 5 -Name "search_code" -Arguments @{ repo = $repoName; query = "reindexOrders"; limit = 5 }
    $search = Invoke-McpTool -Id 6 -Name "query" -Arguments @{ repo = $repoName; query = "OrderController"; limit = 5 }
    $context = Invoke-McpTool -Id 7 -Name "context" -Arguments @{ repo = $repoName; symbol = "OrderService" }

    $listText = $listRepos | ConvertTo-Json -Depth 32 -Compress
    $searchCodeText = $searchCode | ConvertTo-Json -Depth 32 -Compress
    $searchText = $search | ConvertTo-Json -Depth 32 -Compress
    $contextText = $context | ConvertTo-Json -Depth 32 -Compress
    if ($listText -notmatch $repoName) {
        throw "list_repos output did not include $repoName"
    }
    if ($searchCodeText -notmatch "reindexOrders") {
        throw "search_code output did not include reindexOrders"
    }
    if ($searchText -notmatch "OrderController") {
        throw "query output did not include OrderController"
    }
    if ($contextText -notmatch "OrderService") {
        throw "context output did not include OrderService"
    }

    $logTail = Get-DesktopLogTail
    if ($logTail -notmatch "desktop runtime configured") {
        throw "desktop log did not include runtime configuration marker"
    }
    if ($logTail -notmatch "desktop MCP server started") {
        throw "desktop log did not include MCP startup marker"
    }
    if ($logTail -notmatch "source=embedded-dll") {
        throw "desktop log did not show Windows direct-facts embedded DLL path"
    }
    if ($logTail -match "falling back to binary engine") {
        throw "desktop log shows embedded engine fallback despite require mode"
    }

    [PSCustomObject]@{
        ok = $true
        portableDir = $portable
        productShape = "single AKA.exe with materialized embedded engine DLL"
        repo = $repoName
        repoPath = $repo
        akaHome = $akaHome
        mcpServer = $init.result.serverInfo
        mcpSession = [bool]$script:McpSessionId
        stats = @{
            nodes = $readyRepo.nodes
            edges = $readyRepo.edges
            status = $readyRepo.status
        }
        desktopProcessAlive = (-not $desktop.HasExited)
        desktopMcpPort = 4112
        searchCodeMatched = ($searchCodeText -match "reindexOrders")
        queryMatched = ($searchText -match "OrderController")
        contextMatched = ($contextText -match "OrderService")
    } | ConvertTo-Json -Depth 12
} finally {
    if ($desktop -and !$desktop.HasExited) {
        Stop-Process -Id $desktop.Id -Force -ErrorAction SilentlyContinue
    }
}
