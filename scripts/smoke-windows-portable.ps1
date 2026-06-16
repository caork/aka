param(
    [Parameter(Mandatory = $true)]
    [string]$PortableDir,

    [string]$WorkDir = "$env:TEMP\aka-windows-smoke"
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$portable = (Resolve-Path $PortableDir).Path
$akaExe = Join-Path $portable "AKA.exe"
$engineExe = Join-Path $portable "engine\aka-engine.exe"
if (!(Test-Path $akaExe)) {
    throw "Missing AKA.exe in $portable"
}
if (!(Test-Path $engineExe)) {
    throw "Missing engine\aka-engine.exe in $portable"
}

Remove-Item -Recurse -Force $WorkDir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $WorkDir | Out-Null
$repo = Join-Path $WorkDir "spring-demo"
$akaHome = Join-Path $WorkDir "aka-home"
New-Item -ItemType Directory -Force $repo | Out-Null

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
"@ | Set-Content -Encoding UTF8 (Join-Path $repo "OrderController.java")

@"
package com.example.demo;

public class OrderService {
    public String loadOrder(String id) { return "order-" + id; }
    public void reindexOrders() { loadOrder("boot"); }
}
"@ | Set-Content -Encoding UTF8 (Join-Path $repo "OrderService.java")

function Invoke-Aka {
    param(
        [string[]]$Arguments,
        [string]$Name
    )
    $stdout = Join-Path $WorkDir "$Name.out.txt"
    $stderr = Join-Path $WorkDir "$Name.err.txt"
    $env:AKA_HOME = $akaHome
    $process = Start-Process `
        -FilePath $akaExe `
        -ArgumentList $Arguments `
        -NoNewWindow `
        -Wait `
        -PassThru `
        -RedirectStandardOutput $stdout `
        -RedirectStandardError $stderr
    $out = if (Test-Path $stdout) { Get-Content $stdout -Raw } else { "" }
    $err = if (Test-Path $stderr) { Get-Content $stderr -Raw } else { "" }
    if ($process.ExitCode -ne 0) {
        throw "$Name failed with exit code $($process.ExitCode)`nSTDOUT:`n$out`nSTDERR:`n$err"
    }
    [PSCustomObject]@{
        ExitCode = $process.ExitCode
        Stdout = $out
        Stderr = $err
    }
}

$engineVersion = & $engineExe --version
$analyze = Invoke-Aka -Name "analyze" -Arguments @("analyze", $repo)
$registryPath = Join-Path $akaHome "registry.json"
if (!(Test-Path $registryPath)) {
    throw "Missing registry.json after analyze"
}
$registry = Get-Content $registryPath -Raw | ConvertFrom-Json
if (!$registry.repos -or $registry.repos.Count -lt 1) {
    throw "registry.json has no repos"
}
$entry = $registry.repos[0]
$dataDir = $entry.dataDir
$required = @(
    "artifact\manifest.json",
    "artifact\nodes.ndjson",
    "artifact\edges.ndjson",
    "artifact\chunks.ndjson",
    "graph.db",
    "search\meta.json"
)
foreach ($relative in $required) {
    $path = Join-Path $dataDir $relative
    if (!(Test-Path $path)) {
        throw "Missing smoke artifact: $path"
    }
}

$searchCode = Invoke-Aka -Name "search-code" -Arguments @("search-code", "reindexOrders", "--limit", "5")
if ($searchCode.Stdout -notmatch "reindexOrders") {
    throw "search-code output did not include reindexOrders"
}

$search = Invoke-Aka -Name "search" -Arguments @("search", "OrderController", "--limit", "5")
if ($search.Stdout -notmatch "OrderController") {
    throw "search output did not include OrderController"
}

[PSCustomObject]@{
    ok = $true
    portableDir = $portable
    engineVersion = ($engineVersion -join "`n")
    repoPath = $repo
    akaHome = $akaHome
    stats = $entry.stats
    searchCodeMatched = ($searchCode.Stdout -match "reindexOrders")
    searchMatched = ($search.Stdout -match "OrderController")
} | ConvertTo-Json -Depth 8
