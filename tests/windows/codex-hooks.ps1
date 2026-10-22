param([string]$Exe = 'target/release/clawdtoast.exe')

$ErrorActionPreference = 'Stop'
$PSDefaultParameterValues['Get-Content:Encoding'] = 'UTF8'
$exePath = (Resolve-Path $Exe).Path
$state = Join-Path $env:TEMP ('clawdtoast-codex-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $state | Out-Null

function Start-Hook([string]$eventName, [string]$payload = '{"session_id":"open-pipe-test","cwd":"D:/tests","last_assistant_message":"Finished testing"}') {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $exePath
    $start.Arguments = 'codex-hook ' + $eventName
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.EnvironmentVariables['CLAWDTOAST_DIR'] = $state
    $start.EnvironmentVariables['CLAWDTOAST_CONFIG'] = Join-Path $state 'config.toml'
    $start.EnvironmentVariables['CLAWDTOAST_HEADLESS'] = '1'
    $start.EnvironmentVariables['CLAWDTOAST_MUTE'] = '1'
    $start.EnvironmentVariables['CLAWDTOAST_GRACE_SECS'] = '1'
    $proc = [Diagnostics.Process]::Start($start)
    $out = $proc.StandardOutput.ReadToEndAsync()
    $err = $proc.StandardError.ReadToEndAsync()
    # Deliberately do not write a newline or close stdin: Codex may leave its
    # JSON pipe open. Captured output must also reach EOF despite the daemon.
    $proc.StandardInput.Write($payload)
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
    # A structured question fires early, and a duplicate hook for the same call
    # must not play another sound or create another card.
    $inputPrompt = '{"session_id":"question-test","cwd":"D:/tests","turn_id":"turn-1","tool_use_id":"call-1","tool_name":"request_user_input","tool_input":{"questions":[{"question":"Which approach should I use?"}]}}'
    Check-Hook (Start-Hook 'pre-tool-use' $inputPrompt)
    Check-Hook (Start-Hook 'pre-tool-use' $inputPrompt)
    Check-Hook (Start-Hook 'pre-tool-use' '{"session_id":"question-test","tool_name":"Bash"}')
    Check-Hook (Start-Hook 'stop' '{"session_id":"question-test","turn_id":"turn-1","last_assistant_message":"Which approach should I use?"}')
    Check-Hook (Start-Hook 'stop' '{"session_id":"question-test","turn_id":"turn-2","last_assistant_message":"Can you clarify the requirement?"}')
    Start-Sleep -Milliseconds 250
    $log = Get-Content (Join-Path $state 'clawdtoast.log') -Raw
    $waiting = @($log -split "`n" | Where-Object { $_ -match 'card: Waiting for you' }).Count
    if ($waiting -ne 2 -or $log -notmatch 'Which approach should I use\?' -or $log -notmatch 'Can you clarify the requirement\?') {
        throw "expected two distinct waiting notifications, got $waiting; log:`n$log"
    }
    $asyncPrompt = '{"session_id":"async-test","cwd":"D:/tests","turn_id":"async-turn","tool_use_id":"async-call","tool_name":"functions.request_user_input_async","tool_input":{"questions":[{"title":"Which caf\u00e9?","options":["One","Two"]},{"title":"When should we ship?"}]},"tool_response":"{\"accepted\":true}"}'
    Check-Hook (Start-Hook 'post-tool-use' $asyncPrompt)
    Check-Hook (Start-Hook 'post-tool-use' $asyncPrompt)
    Check-Hook (Start-Hook 'post-tool-use' $asyncPrompt.Replace('true', 'false'))
    Check-Hook (Start-Hook 'user-prompt-submit' '{"session_id":"async-test"}')
    Check-Hook (Start-Hook 'stop' '{"session_id":"async-test","turn_id":"async-turn","last_assistant_message":"Which caf\u00e9?"}')
    Check-Hook (Start-Hook 'stop' '{"session_id":"async-test","turn_id":"async-turn","last_assistant_message":"Finished the current task."}')
    Start-Sleep -Milliseconds 100
    $log = Get-Content (Join-Path $state 'clawdtoast.log') -Raw
    $asyncCards = @($log -split "`n" | Where-Object { $_ -match 'card: Waiting for you.*2 unconfirmed' }).Count
    if ($asyncCards -ne 1 -or $log -notmatch 'Which caf') { throw "accepted async question missing or duplicated: $log" }
    $checkpointPath = Join-Path $state 'codex-questions.json'
    $saved = Get-Content $checkpointPath -Raw | ConvertFrom-Json
    if (@($saved.entries | Where-Object { $_.session -eq 'codex:async-test' }).Count -ne 2) { throw 'working or completion incorrectly cleared async questions' }
    if ((Get-Content $checkpointPath -Raw) -match 'Which caf|When should') { throw 'question text leaked into restart ledger' }
    # A real daemon restart must preserve duplicate suppression without re-alerting.
    Stop-TestDaemon $state
    Check-Hook (Start-Hook 'post-tool-use' $asyncPrompt)
    Start-Sleep -Milliseconds 100
    $log = Get-Content (Join-Path $state 'clawdtoast.log') -Raw
    if (@($log -split "`n" | Where-Object { $_ -match 'card: Waiting for you.*2 unconfirmed' }).Count -ne 1) { throw 'daemon restart replayed async alert' }
    Check-Hook (Start-Hook 'post-tool-use' $asyncPrompt.Replace('async-call', 'async-call-2').Replace('"{\"accepted\":true}"', '{"accepted":true}'))
    Start-Sleep -Milliseconds 100
    $log = Get-Content (Join-Path $state 'clawdtoast.log') -Raw
    if ($log -notmatch 'card: Waiting for you.*4 unconfirmed') { throw 'second accepted call was lost' }
    Check-Hook (Start-Hook 'session-end' '{"session_id":"async-test"}')
    Start-Sleep -Milliseconds 100
    $saved = Get-Content $checkpointPath -Raw | ConvertFrom-Json
    if (@($saved.entries | Where-Object { $_.session -eq 'codex:async-test' }).Count -ne 0) { throw 'SessionEnd retained async questions' }
    Stop-TestDaemon $state
    [IO.File]::WriteAllText((Join-Path $state 'config.toml'), "[events]`nasync_questions=false`n")
    Check-Hook (Start-Hook 'post-tool-use' $asyncPrompt.Replace('async-test', 'disabled-test'))
    Start-Sleep -Milliseconds 100
    $log = Get-Content (Join-Path $state 'clawdtoast.log') -Raw
    $before = @($log -split "`n" | Where-Object { $_ -match 'card: Waiting for you' }).Count
    if ($before -ne 4) { throw 'disabled async category produced a card' }
    Stop-TestDaemon $state
    [IO.File]::WriteAllText((Join-Path $state 'config.toml'), "[attention]`nshow_question_text=false`n")
    Check-Hook (Start-Hook 'post-tool-use' $asyncPrompt.Replace('async-test', 'private-test').Replace('When should we ship?', 'PRIVATE_TITLE_SENTINEL'))
    Start-Sleep -Milliseconds 100
    $log = Get-Content (Join-Path $state 'clawdtoast.log') -Raw
    if ($log -match 'PRIVATE_TITLE_SENTINEL' -or $log -notmatch 'Codex has requested your input') { throw 'question privacy preference was not applied' }
    # Exercise the Windows filesystem installer, with a temporary Codex home.
    $previousCodexHome = $env:CODEX_HOME
    try {
        $env:CODEX_HOME = Join-Path $state 'codex-home'
        New-Item -ItemType Directory -Path $env:CODEX_HOME | Out-Null
        $hooksPath = Join-Path $env:CODEX_HOME 'hooks.json'
        $original = '{"description":"preserve","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo other"}]}]}}'
        [IO.File]::WriteAllText($hooksPath, $original)
        & $exePath install codex | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'Windows Codex install failed' }
        $installed = Get-Content $hooksPath -Raw
        & $exePath install codex | Out-Null
        if ($installed -ne (Get-Content $hooksPath -Raw)) { throw 'Codex install was not idempotent' }
        $hooks = $installed | ConvertFrom-Json
        if (@($hooks.hooks.PSObject.Properties).Count -ne 9 -or $hooks.hooks.PostToolUse[0].hooks[0].async -ne $true) { throw 'installer omitted required async hooks' }
        & $exePath uninstall codex | Out-Null
        $restored = Get-Content $hooksPath -Raw | ConvertFrom-Json
        if ($restored.description -ne 'preserve' -or @($restored.hooks.PSObject.Properties).Count -ne 1 -or
            $restored.hooks.Stop.Count -ne 1 -or $restored.hooks.Stop[0].hooks.Count -ne 1 -or
            $restored.hooks.Stop[0].hooks[0].command -ne 'echo other' -or $restored.hooks.Stop[0].hooks[0].type -ne 'command') {
            throw 'Windows uninstall did not preserve original settings'
        }
    } finally { $env:CODEX_HOME = $previousCodexHome }
    Write-Host 'Async acceptance, multi-question counts, restart deduplication, privacy and session cleanup passed.'
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
