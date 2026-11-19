# Older builds do not acknowledge the shutdown request. Limit the fallback
# to the executable paired with this window in the same Windows session.
$ErrorActionPreference = 'Stop'
$session = [Diagnostics.Process]::GetCurrentProcess().SessionId
foreach ($engine in @(Get-Process -Name orvia-engine -ErrorAction SilentlyContinue)) {
    if ($engine.SessionId -ne $session -or $engine.Path -ne $env:ORVIA_ENGINE_PATH) { continue }
    $request = Join-Path $env:ORVIA_STATE_DIR ("stop-engine-" + $engine.Id)
    try {
        [IO.File]::WriteAllText($request, '')
        if (!$engine.WaitForExit(1500)) {
            $engine.Kill()
            if (!$engine.WaitForExit(5000)) { throw 'The audio engine did not exit.' }
        }
    } finally {
        [IO.File]::Delete($request)
    }
}
