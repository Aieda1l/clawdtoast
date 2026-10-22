param([string]$Exe = 'target/release/clawdtoast.exe')

$ErrorActionPreference = 'Stop'
$exePath = (Resolve-Path $Exe).Path
$state = Join-Path $env:TEMP ('clawdtoast-codex-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $state | Out-Null

function Start-Hook([string]$eventName) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $exePath
    $start.Arguments = 'codex-hook ' + $eventName
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.EnvironmentVariables['CLAWDTOAST_DIR'] = $state
    $start.EnvironmentVariables['CLAWDTOAST_HEADLESS'] = '1'
    $start.EnvironmentVariables['CLAWDTOAST_MUTE'] = '1'
    $start.EnvironmentVariables['CLAWDTOAST_GRACE_SECS'] = '1'
    $proc = [Diagnostics.Process]::Start($start)
    $out = $proc.StandardOutput.ReadToEndAsync()
    $err = $proc.StandardError.ReadToEndAsync()
    # Deliberately do not write a newline or close stdin: Codex may leave its
    # JSON pipe open. Captured output must also reach EOF despite the daemon.
    $proc.StandardInput.Write('{"session_id":"open-pipe-test","cwd":"D:/tests","last_assistant_message":"Finished testing"}')
    $proc.StandardInput.Flush()
    return @{ Event = $eventName; Process = $proc; Out = $out; Err = $err }
}

function Check-Hook($hook) {
    $p = $hook.Process
    try {
        if (!$p.WaitForExit(4000)) {
            $p.Kill()
            throw "$($hook.Event): hook did not exit in 4s"
        }
        if ($p.ExitCode -ne 0) { throw "$($hook.Event): exit $($p.ExitCode)" }
        if (!$hook.Out.Wait(1000) -or !$hook.Err.Wait(1000)) {
            throw "$($hook.Event): child daemon inherited hook output handles"
        }
        if ($hook.Event -eq 'stop' -and $hook.Out.Result.Trim() -ne '{}') {
            throw "stop hook must return an empty JSON object"
        }
    } finally {
        $p.Dispose()
    }
}

function Start-FileInputHook {
    # cmd.exe's `<` passes a disk file handle to the hook, unlike .NET's
    # RedirectStandardInput, which always creates a pipe.
    $inputPath = Join-Path $state 'hook-input.json'
    [IO.File]::WriteAllText($inputPath, '{"session_id":"file-input-test","cwd":"D:/tests","last_assistant_message":"File-backed hook input."}')
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = 'cmd.exe'
    $start.Arguments = '/d /s /c ""' + $exePath + '" codex-hook stop < "' + $inputPath + '""'
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.EnvironmentVariables['CLAWDTOAST_DIR'] = $state
    $start.EnvironmentVariables['CLAWDTOAST_HEADLESS'] = '1'
    $start.EnvironmentVariables['CLAWDTOAST_MUTE'] = '1'
    $start.EnvironmentVariables['CLAWDTOAST_GRACE_SECS'] = '1'
    $proc = [Diagnostics.Process]::Start($start)
    return @{ Event = 'stop'; Process = $proc; Out = $proc.StandardOutput.ReadToEndAsync(); Err = $proc.StandardError.ReadToEndAsync() }
}

function Stop-TestDaemon([string]$directory) {
    $portPath = Join-Path $directory 'clawdtoast.sock'
    if (!(Test-Path -LiteralPath $portPath)) { return }
    $client = [Net.Sockets.TcpClient]::new()
    try {
        $client.Connect('127.0.0.1', [int](Get-Content -LiteralPath $portPath -Raw))
        $bytes = [Text.Encoding]::UTF8.GetBytes("{`"v`":1,`"type`":`"shutdown`"}`n")
        $client.GetStream().Write($bytes, 0, $bytes.Length)
    } finally {
        $client.Dispose()
    }
    $deadline = [DateTime]::UtcNow.AddSeconds(4)
    while ((Test-Path -LiteralPath $portPath) -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 20
    }
    if (Test-Path -LiteralPath $portPath) { throw "test daemon did not shut down: $directory" }
}

function Test-PowerShellHook([string]$eventName) {
    # Codex can launch hooks through Windows PowerShell. It passes extra
    # inheritable copies of its capture pipes to native programs, beyond stdio.
    # Each case needs a cold daemon; a warm daemon cannot reproduce the leak.
    $shellState = Join-Path $state ('powershell-' + $eventName)
    New-Item -ItemType Directory -Path $shellState | Out-Null
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $start.Arguments = '-NoProfile -NonInteractive -Command "& ''' + $exePath.Replace("'", "''") + ''' codex-hook ' + $eventName + '"'
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.EnvironmentVariables['CLAWDTOAST_DIR'] = $shellState
    $start.EnvironmentVariables['CLAWDTOAST_HEADLESS'] = '1'
    $start.EnvironmentVariables['CLAWDTOAST_MUTE'] = '1'
    $start.EnvironmentVariables['CLAWDTOAST_GRACE_SECS'] = '1'
    $proc = [Diagnostics.Process]::Start($start)
    $hook = @{ Event = $eventName; Process = $proc; Out = $proc.StandardOutput.ReadToEndAsync(); Err = $proc.StandardError.ReadToEndAsync() }
    try {
        $proc.StandardInput.Write('{"session_id":"powershell-test","cwd":"D:/tests","tool_name":"Bash","last_assistant_message":"PowerShell hook completed."}')
        $proc.StandardInput.Flush()
        # Check EOF while the daemon is still running, before shutting it down.
        Check-Hook $hook
        $log = Get-Content -LiteralPath (Join-Path $shellState 'clawdtoast.log') -Raw
        if ($log -notmatch 'daemon up') { throw "$eventName did not start a daemon through PowerShell" }
        if ($eventName -eq 'stop' -and $log -notmatch 'card: Done') { throw 'PowerShell stop notification was lost' }
        if ($eventName -eq 'permission-request' -and $log -notmatch 'card: Needs permission') { throw 'PowerShell permission notification was lost' }
    } finally {
        Stop-TestDaemon $shellState
    }
}

try {
    foreach ($eventName in @('session-start', 'permission-request', 'stop')) {
        Test-PowerShellHook $eventName
    }
    # Several hooks starting together should not lose alerts or hold pipes open.
    $startHooks = @(1..6 | ForEach-Object { Start-Hook 'session-start' })
    foreach ($hook in $startHooks) { Check-Hook $hook }
    $stopHooks = @(1..10 | ForEach-Object { Start-Hook 'stop' })
    foreach ($hook in $stopHooks) { Check-Hook $hook }
    Check-Hook (Start-FileInputHook)
    Check-Hook (Start-Hook 'session-end')

    $log = Get-Content (Join-Path $state 'clawdtoast.log') -Raw
    $cards = @($log -split "`n" | Where-Object { $_ -match 'card: Done' }).Count
    if ($cards -ne 11 -or $log -notmatch 'File-backed hook input') {
        throw "expected 11 notifications including file-backed input, got $cards; log:`n$log"
    }
    if ($log -match 'timeout|did not come up|could not show') { throw "hook error: $log" }
    Write-Host 'Codex PowerShell, open-pipe, file-input and concurrent hook smoke test passed (11/11 concurrent cards).'
} finally {
    Stop-TestDaemon $state
    $resolvedState = [IO.Path]::GetFullPath($state)
    $tempRoot = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\'
    if (!$resolvedState.StartsWith($tempRoot + 'clawdtoast-codex-test-', [StringComparison]::OrdinalIgnoreCase)) {
        throw "refusing to remove unexpected test directory: $resolvedState"
    }
    Remove-Item -LiteralPath $resolvedState -Recurse -Force
}
