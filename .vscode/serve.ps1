# Kill this workspace's quota-dash server, start a new one, open the browser.
param(
    [Parameter(Mandatory = $true)]
    [string]$Workspace,
    [switch]$Watch,
    [string]$ExpectedPid = "",
    [string]$Generation = "",
    [string]$Url = "",
    [string]$ProbeHost = "127.0.0.1",
    [int]$Port = 0
)

$ErrorActionPreference = "Stop"

$root = (Resolve-Path -LiteralPath $Workspace).Path
$target = Join-Path $root "target"
$pidFile = Join-Path $target "quota-dash-serve.pid"
$genFile = Join-Path $target "quota-dash-serve.generation"
$outLog = Join-Path $target "quota-dash-serve.out.log"
$errLog = Join-Path $target "quota-dash-serve.err.log"
$configPath = Join-Path $root "quota-dash.toml"
$exePrefix = Join-Path $root "target\"

function Read-Text([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return "" }
    return [string](Get-Content -LiteralPath $Path -ErrorAction SilentlyContinue | Select-Object -First 1)
}

function Still-Current {
    return (Read-Text $genFile) -eq $Generation -and (Read-Text $pidFile) -eq $ExpectedPid
}

function Test-Listening([string]$HostName, [int]$ListenPort) {
    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $wait = $client.BeginConnect($HostName, $ListenPort, $null, $null)
        if (-not $wait.AsyncWaitHandle.WaitOne(300)) { return $false }
        $client.EndConnect($wait) | Out-Null
        return $client.Connected
    }
    catch {
        return $false
    }
    finally {
        $client.Close()
    }
}

function Show-StartupFailure([string]$Reason) {
    if (-not (Still-Current)) { return }
    $message = "quota-dash did not start. $Reason`r`n`r`nLog: $errLog"
    Start-Process -FilePath "powershell.exe" -ArgumentList @(
        "-NoExit",
        "-NoProfile",
        "-Command",
        "Write-Host '$($message.Replace("'", "''"))'; Write-Host ''; if (Test-Path -LiteralPath '$errLog') { Get-Content -LiteralPath '$errLog' -Tail 40 }; if (Test-Path -LiteralPath '$outLog') { Get-Content -LiteralPath '$outLog' -Tail 20 }"
    )
}

if ($Watch) {
    $deadline = (Get-Date).AddMinutes(10)
    while ((Get-Date) -lt $deadline) {
        if (-not (Still-Current)) { exit 0 }
        $alive = Get-Process -Id ([int]$ExpectedPid) -ErrorAction SilentlyContinue
        if (-not $alive) {
            Show-StartupFailure "The server process exited before it accepted connections."
            exit 1
        }
        if (Test-Listening $ProbeHost $Port) {
            if (-not (Still-Current)) { exit 0 }
            Start-Process $Url
            exit 0
        }
        Start-Sleep -Seconds 1
    }
    Show-StartupFailure "Timed out waiting for $Url"
    exit 1
}

function Read-DashEndpoint {
    $bind = "127.0.0.1"
    $port = 8787
    if (-not (Test-Path -LiteralPath $configPath)) {
        return @{ Bind = $bind; Port = $port }
    }
    foreach ($line in [System.IO.File]::ReadLines($configPath)) {
        $trim = $line.Trim()
        if ($trim.Length -eq 0 -or $trim.StartsWith("#")) { continue }
        if ($trim -match '^bind\s*=\s*"([^"]*)"') {
            if ($Matches[1].Length -gt 0) { $bind = $Matches[1] }
        }
        elseif ($trim -match '^port\s*=\s*(\d+)') {
            $parsed = [int]$Matches[1]
            if ($parsed -gt 0) { $port = $parsed }
        }
    }
    return @{ Bind = $bind; Port = $port }
}

function Get-OpenUrl([string]$BindHost, [int]$ListenPort) {
    $hostForUrl = $BindHost
    if ($BindHost -eq "0.0.0.0" -or $BindHost -eq "::" -or $BindHost -eq "[::]") {
        $hostForUrl = "127.0.0.1"
    }
    if ($hostForUrl.Contains(":")) {
        return "http://[$hostForUrl]:$ListenPort/"
    }
    return "http://${hostForUrl}:$ListenPort/"
}

function Stop-Tree([int]$ProcessId) {
    if ($ProcessId -le 0) { return }
    & taskkill.exe /PID $ProcessId /T /F *> $null
}

function Stop-PreviousServer {
    New-Item -ItemType Directory -Force -Path $target | Out-Null

    $watchers = Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" -ErrorAction SilentlyContinue
    foreach ($watcher in $watchers) {
        $cmd = [string]$watcher.CommandLine
        $isOurs = $cmd.IndexOf($root, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and
            $cmd.IndexOf("-Watch", [System.StringComparison]::OrdinalIgnoreCase) -ge 0
        if ($isOurs) {
            Stop-Process -Id $watcher.ProcessId -Force -ErrorAction SilentlyContinue
        }
    }

    $recorded = Read-Text $pidFile
    if ($recorded -match '^\d+$') {
        $old = Get-CimInstance Win32_Process -Filter "ProcessId = $recorded" -ErrorAction SilentlyContinue
        if ($old) {
            $cmd = [string]$old.CommandLine
            $exe = [string]$old.ExecutablePath
            $ours = $cmd.IndexOf($configPath, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
                $exe.StartsWith($exePrefix, [System.StringComparison]::OrdinalIgnoreCase)
            if ($ours) {
                Write-Host "Stopping previous server (pid $recorded)"
                Stop-Tree ([int]$recorded)
            }
        }
    }

    $cargo = Get-CimInstance Win32_Process -Filter "Name = 'cargo.exe'" -ErrorAction SilentlyContinue
    foreach ($proc in $cargo) {
        $cmd = [string]$proc.CommandLine
        $isServer = $cmd.IndexOf($configPath, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and
            $cmd.IndexOf(" run ", [System.StringComparison]::OrdinalIgnoreCase) -ge 0
        if ($isServer) {
            Write-Host "Stopping cargo (pid $($proc.ProcessId))"
            Stop-Tree ([int]$proc.ProcessId)
        }
    }

    $bins = Get-CimInstance Win32_Process -Filter "Name = 'quota_dash.exe'" -ErrorAction SilentlyContinue
    foreach ($proc in $bins) {
        $exe = [string]$proc.ExecutablePath
        if ($exe.StartsWith($exePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            Write-Host "Stopping quota_dash (pid $($proc.ProcessId))"
            Stop-Tree ([int]$proc.ProcessId)
        }
    }

    Start-Sleep -Milliseconds 400
}

$endpoint = Read-DashEndpoint
$url = Get-OpenUrl $endpoint.Bind $endpoint.Port
$probe = "127.0.0.1"
if ($endpoint.Bind -ne "0.0.0.0" -and $endpoint.Bind -ne "::" -and $endpoint.Bind -ne "[::]") {
    $probe = $endpoint.Bind.Trim("[]")
}

$cargoCmd = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargoCmd) {
    Write-Host "cargo was not found on PATH. Install Rust from https://rustup.rs/ and restart Cursor."
    exit 1
}

$generation = [guid]::NewGuid().ToString("N")
New-Item -ItemType Directory -Force -Path $target | Out-Null
Set-Content -LiteralPath $genFile -Value $generation -Encoding ascii

Stop-PreviousServer

foreach ($log in @($outLog, $errLog, $pidFile)) {
    if (Test-Path -LiteralPath $log) {
        Remove-Item -LiteralPath $log -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "Starting quota-dash at $url"
Write-Host "cargo run --release"
Write-Host "The browser opens when the server is listening."

$proc = Start-Process -FilePath $cargoCmd.Source `
    -ArgumentList @("run", "--release", "--", $configPath) `
    -WorkingDirectory $root `
    -WindowStyle Hidden `
    -RedirectStandardOutput $outLog `
    -RedirectStandardError $errLog `
    -PassThru

Set-Content -LiteralPath $pidFile -Value $proc.Id -Encoding ascii

Start-Process -FilePath "powershell.exe" -ArgumentList @(
    "-NoProfile",
    "-ExecutionPolicy", "Bypass",
    "-WindowStyle", "Hidden",
    "-File", (Join-Path $root ".vscode\serve.ps1"),
    "-Workspace", $root,
    "-Watch",
    "-ExpectedPid", "$($proc.Id)",
    "-Generation", $generation,
    "-Url", $url,
    "-ProbeHost", $probe,
    "-Port", "$($endpoint.Port)"
) -WindowStyle Hidden | Out-Null

exit 0
