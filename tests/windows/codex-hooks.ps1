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
    $start.Environment['CLAWDTOAST_DIR'] = $state
    $start.Environment['CLAWDTOAST_HEADLESS'] = '1'
    $start.Environment['CLAWDTOAST_MUTE'] = '1'
    $start.Environment['CLAWDTOAST_GRACE_SECS'] = '1'
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
    $start.Environment['CLAWDTOAST_DIR'] = $state
    $start.Environment['CLAWDTOAST_HEADLESS'] = '1'
    $start.Environment['CLAWDTOAST_MUTE'] = '1'
    $start.Environment['CLAWDTOAST_GRACE_SECS'] = '1'
    $proc = [Diagnostics.Process]::Start($start)
    return @{ Event = 'stop'; Process = $proc; Out = $proc.StandardOutput.ReadToEndAsync(); Err = $proc.StandardError.ReadToEndAsync() }
}

try {
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
    Write-Host 'Codex open-pipe, file-input and concurrent hook smoke test passed (11/11 cards).'
} finally {
    Start-Sleep -Seconds 2
    Remove-Item -Path $state -Recurse -Force -ErrorAction SilentlyContinue
}
