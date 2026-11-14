[CmdletBinding()]
param([string]$Executable)
$ErrorActionPreference = 'Stop'
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if (!$Executable) { $Executable = Join-Path $repositoryRoot 'native/target/release/MoaLauncher.exe' }
$Executable = (Resolve-Path -LiteralPath $Executable).Path
$fixtureRoot = Join-Path $repositoryRoot ('artifacts/native-lifecycle/' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixtureRoot -Force | Out-Null
@{ checkOnStartup = $false; periodicChecks = $false; updateNotifications = $false } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $fixtureRoot 'settings.json') -Encoding utf8

function Start-Launcher([bool]$Background) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Executable
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    if ($Background) { $start.ArgumentList.Add('--background') }
    $start.ArgumentList.Add('--data-dir')
    $start.ArgumentList.Add($fixtureRoot)
    [Diagnostics.Process]::Start($start)
}

$launcher = Start-Launcher $true
$second = $null
try {
    # Observe the whole startup period, including eframe's usual first-frame show.
    $deadline = [DateTime]::UtcNow.AddSeconds(3)
    while ([DateTime]::UtcNow -lt $deadline) {
        $launcher.Refresh()
        if ($launcher.HasExited) { throw 'Background launcher exited unexpectedly.' }
        if ($launcher.MainWindowHandle -ne [IntPtr]::Zero) { throw 'Background launch displayed a window.' }
        Start-Sleep -Milliseconds 100
    }
    $second = Start-Launcher $false
    if (!$second.WaitForExit(15000) -or $second.ExitCode -ne 0) { throw 'Second launch did not activate the existing instance.' }
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    do {
        $launcher.Refresh()
        if ($launcher.HasExited) { throw 'Original launcher exited during activation.' }
        if ($launcher.MainWindowHandle -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($launcher.MainWindowHandle -eq [IntPtr]::Zero) { throw 'The existing process did not create its window after activation.' }
    [pscustomobject]@{ BackgroundStayedHidden = $true; SameProcessActivated = $true; WindowTitle = $launcher.MainWindowTitle } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $fixtureRoot 'result.json') -Encoding utf8
    Write-Output 'BackgroundStayedHidden=true SameProcessActivated=true'
}
finally {
    foreach ($ownedProcess in @($second, $launcher)) { if ($ownedProcess -and !$ownedProcess.HasExited) { $ownedProcess.Kill($true); $ownedProcess.WaitForExit() } }
}
