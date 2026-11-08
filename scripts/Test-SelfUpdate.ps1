[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PublishedDirectory,
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,
    [string]$OptionalBaselineDirectory
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$artifactRoot = [IO.Path]::GetFullPath((Join-Path $repositoryRoot 'artifacts'))
$validationRoot = [IO.Path]::GetFullPath((Join-Path $artifactRoot 'self-update-validation'))
$runId = [Guid]::NewGuid().ToString('N')
$runRoot = [IO.Path]::GetFullPath((Join-Path $validationRoot "run-$runId"))
$launcherName = 'MoaLauncher.exe'
$script:ownedProcesses = [Collections.Generic.List[object]]::new()

function Assert-UnderRoot([string]$Root, [string]$Path) {
    $fullRoot = [IO.Path]::GetFullPath($Root).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
    $fullPath = [IO.Path]::GetFullPath($Path)
    if (!$fullPath.StartsWith($fullRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to access a path outside its allowed root: $fullPath"
    }
    return $fullPath
}

function Get-FullInputPath([string]$Path, [string]$Description) {
    if ([string]::IsNullOrWhiteSpace($Path)) { throw "$Description is required." }
    return [IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Path).Path)
}

function Test-SamePath([string]$First, [string]$Second) {
    return [string]::Equals(
        [IO.Path]::GetFullPath($First).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar),
        [IO.Path]::GetFullPath($Second).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar),
        [StringComparison]::OrdinalIgnoreCase)
}

function Copy-Tree([string]$Source, [string]$Destination) {
    $sourceRoot = [IO.Path]::GetFullPath($Source).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
    $destinationRoot = Assert-UnderRoot $runRoot $Destination
    [void](New-Item -ItemType Directory -Path $destinationRoot -Force)
    $pending = [Collections.Generic.Stack[string]]::new()
    $pending.Push($sourceRoot)
    while ($pending.Count -gt 0) {
        $sourceDirectory = $pending.Pop()
        $relativeDirectory = [IO.Path]::GetRelativePath($sourceRoot, $sourceDirectory)
        $destinationDirectory = if ($relativeDirectory -eq '.') { $destinationRoot } else { Join-Path $destinationRoot $relativeDirectory }
        $destinationDirectory = Assert-UnderRoot $runRoot $destinationDirectory
        [void](New-Item -ItemType Directory -Path $destinationDirectory -Force)

        foreach ($entry in Get-ChildItem -LiteralPath $sourceDirectory -Force) {
            if (($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Refusing to copy a reparse point from the source tree: $($entry.FullName)"
            }
            if ($entry.PSIsContainer) {
                $pending.Push($entry.FullName)
                continue
            }
            $relativeFile = [IO.Path]::GetRelativePath($sourceRoot, $entry.FullName)
            $destinationFile = Assert-UnderRoot $runRoot (Join-Path $destinationRoot $relativeFile)
            [void](New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($destinationFile)) -Force)
            Copy-Item -LiteralPath $entry.FullName -Destination $destinationFile -Force
        }
    }
}

function Copy-ManifestFiles([string]$Source, [string]$Destination) {
    $sourceRoot = [IO.Path]::GetFullPath($Source).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
    $destinationRoot = Assert-UnderRoot $runRoot $Destination
    [void](New-Item -ItemType Directory -Path $destinationRoot -Force)
    $manifestPath = Join-Path $sourceRoot 'launcher-files.json'
    if (!(Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw "Launcher file manifest is missing: $manifestPath" }
    $manifest = @(Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json)
    if ($manifest.Count -eq 0) { throw 'Launcher file manifest is empty.' }
    $copied = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($relative in $manifest) {
        if ($relative -isnot [string] -or [string]::IsNullOrWhiteSpace($relative)) { throw 'Launcher file manifest contains an invalid path.' }
        $normalizedRelative = $relative.Replace('/', [IO.Path]::DirectorySeparatorChar).Replace('\', [IO.Path]::DirectorySeparatorChar)
        if ([IO.Path]::IsPathRooted($normalizedRelative) -or $normalizedRelative.Contains(':')) { throw "Rooted launcher manifest path is not allowed: $relative" }
        $sourceFile = [IO.Path]::GetFullPath((Join-Path $sourceRoot $normalizedRelative))
        if (!$sourceFile.StartsWith($sourceRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Launcher manifest path escaped the published directory: $relative"
        }
        if (!(Test-Path -LiteralPath $sourceFile -PathType Leaf)) { throw "Launcher manifest file is missing: $relative" }
        $attributes = [IO.File]::GetAttributes($sourceFile)
        if (($attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Launcher manifest file is a reparse point: $relative" }
        $destinationFile = Assert-UnderRoot $runRoot (Join-Path $destinationRoot $normalizedRelative)
        [void](New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($destinationFile)) -Force)
        Copy-Item -LiteralPath $sourceFile -Destination $destinationFile -Force
        [void]$copied.Add($normalizedRelative.Replace('\', '/'))
    }
    foreach ($required in @($launcherName, 'MoaLauncher.dll', 'launcher-files.json')) {
        if (!$copied.Contains($required)) { throw "Launcher manifest does not include required file '$required'." }
    }
}

function Start-HiddenProcess([string]$Executable, [string[]]$Arguments, [string]$WorkingDirectory) {
    $executablePath = Assert-UnderRoot $runRoot $Executable
    if (!(Test-Path -LiteralPath $executablePath -PathType Leaf)) { throw "Executable was not found: $executablePath" }
    $start = [Diagnostics.ProcessStartInfo]::new($executablePath)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.WorkingDirectory = [IO.Path]::GetFullPath($WorkingDirectory)
    foreach ($argument in $Arguments) { [void]$start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($start)
    if ($null -eq $process) { throw "Could not start process: $executablePath" }
    $process.Refresh()
    $script:ownedProcesses.Add([pscustomobject]@{
        Process = $process
        ProcessId = $process.Id
        ExecutablePath = $executablePath
        StartTimeUtcTicks = $process.StartTime.ToUniversalTime().Ticks
    })
    return $process
}

function Test-OwnedProcess([object]$Entry) {
    try {
        $process = $Entry.Process
        $process.Refresh()
        if ($process.HasExited) { return $false }
        $actualPath = $process.MainModule.FileName
        $startTime = $process.StartTime.ToUniversalTime().Ticks
        return (Test-SamePath $actualPath $Entry.ExecutablePath) -and
            ([Math]::Abs($startTime - [long]$Entry.StartTimeUtcTicks) -le [TimeSpan]::FromSeconds(2).Ticks)
    }
    catch { return $false }
}

function Find-TestRestart([string]$Executable, [string]$DataDirectory) {
    $expectedExe = [IO.Path]::GetFullPath($Executable)
    $expectedData = [IO.Path]::GetFullPath($DataDirectory)
    try {
        $processes = Get-CimInstance -ClassName Win32_Process -Filter "Name='$launcherName'" -ErrorAction Stop
        foreach ($candidate in $processes) {
            if ([string]::IsNullOrWhiteSpace($candidate.ExecutablePath) -or
                !(Test-SamePath $candidate.ExecutablePath $expectedExe) -or
                [string]::IsNullOrWhiteSpace($candidate.CommandLine) -or
                !$candidate.CommandLine.Contains('--data-dir', [StringComparison]::OrdinalIgnoreCase) -or
                !$candidate.CommandLine.Contains($expectedData, [StringComparison]::OrdinalIgnoreCase)) { continue }
            $process = [Diagnostics.Process]::GetProcessById([int]$candidate.ProcessId)
            $process.Refresh()
            if ($process.HasExited -or !(Test-SamePath $process.MainModule.FileName $expectedExe)) { $process.Dispose(); continue }
            $entry = [pscustomobject]@{
                Process = $process
                ProcessId = $process.Id
                ExecutablePath = $expectedExe
                StartTimeUtcTicks = $process.StartTime.ToUniversalTime().Ticks
            }
            if (!(@($script:ownedProcesses | Where-Object { $_.ProcessId -eq $entry.ProcessId }).Count -gt 0)) {
                $script:ownedProcesses.Add($entry)
            }
            return $entry
        }
    }
    catch { }
    return $null
}

function Wait-TestRestart([string]$Executable, [string]$DataDirectory, [int]$TimeoutSeconds) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        $entry = Find-TestRestart $Executable $DataDirectory
        if ($null -ne $entry) { return $entry }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "The updated launcher did not restart from the test target within $TimeoutSeconds seconds."
}

function Start-Case([string]$Name, [string]$SourceTarget, [string]$InstallerSource, [string]$Version, [bool]$ExpectSuccess) {
    $caseRoot = Assert-UnderRoot $runRoot (Join-Path $runRoot $Name)
    $target = Assert-UnderRoot $runRoot (Join-Path $caseRoot 'target')
    $data = Assert-UnderRoot $runRoot (Join-Path $caseRoot 'data')
    $jobId = [Guid]::NewGuid().ToString('N')
    $jobRoot = Assert-UnderRoot $runRoot (Join-Path (Join-Path $data 'self-update') $jobId)
    $runner = Assert-UnderRoot $runRoot (Join-Path $jobRoot 'runner')
    [void](New-Item -ItemType Directory -Path $caseRoot -Force)
    Copy-Tree $SourceTarget $target
    [void](New-Item -ItemType Directory -Path $data -Force)
    [void](New-Item -ItemType Directory -Path $jobRoot -Force)
    $userFile = Assert-UnderRoot $runRoot (Join-Path $target 'preserve-user-file.txt')
    Set-Content -LiteralPath $userFile -Value "preserve-$Name-$runId" -Encoding utf8NoBOM
    if (!$ExpectSuccess) {
        $markerPath = Assert-UnderRoot $runRoot (Join-Path $target 'self-update-validation-target.txt')
        Set-Content -LiteralPath $markerPath -Value 'verified isolated failure target' -Encoding utf8NoBOM
    }
    $settingsPath = Assert-UnderRoot $runRoot (Join-Path $data 'settings.json')
    $settingsJson = '{ "checkOnStartup": false, "periodicChecks": false, "checkIntervalMinutes": 1440, "updateNotifications": false }'
    Set-Content -LiteralPath $settingsPath -Value $settingsJson -Encoding utf8NoBOM
    $settingsHashBefore = (Get-FileHash -LiteralPath $settingsPath -Algorithm SHA256).Hash
    $userFileHashBefore = (Get-FileHash -LiteralPath $userFile -Algorithm SHA256).Hash
    $launcherDll = Join-Path $target 'MoaLauncher.dll'
    if (!(Test-Path -LiteralPath $launcherDll -PathType Leaf)) { throw "Target launcher DLL is missing: $launcherDll" }
    $launcherDllHashBefore = (Get-FileHash -LiteralPath $launcherDll -Algorithm SHA256).Hash

    $installerName = "MoaLauncher-$Version-Setup-x64.exe"
    $installer = Assert-UnderRoot $runRoot (Join-Path $jobRoot $installerName)
    Copy-Item -LiteralPath $InstallerSource -Destination $installer
    $sha256 = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
    Copy-ManifestFiles $PublishedDirectory $runner
    $runnerExe = Join-Path $runner $launcherName
    $targetExe = Join-Path $target $launcherName
    $parent = Start-HiddenProcess -Executable $targetExe -Arguments @('--background', '--data-dir', $data) -WorkingDirectory $target
    $parent.Refresh()
    $parentStartTicks = $parent.StartTime.ToUniversalTime().Ticks

    $planPath = Assert-UnderRoot $runRoot (Join-Path $jobRoot 'plan.json')
    $plan = [ordered]@{
        TargetDirectory = $target
        DataDirectory = $data
        InstallerPath = $installer
        Sha256 = $sha256
        Version = $Version
        ParentProcessId = $parent.Id
        ParentStartTimeUtcTicks = $parentStartTicks
        ParentExecutablePath = $targetExe
        Background = $true
    }
    $plan | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $planPath -Encoding utf8NoBOM

    $helper = Start-HiddenProcess -Executable $runnerExe -Arguments @('--apply-self-update', $planPath) -WorkingDirectory $runner
    $helperEntry = $script:ownedProcesses | Where-Object { $_.ProcessId -eq $helper.Id } | Select-Object -First 1
    if ($null -eq $helperEntry) { throw 'The helper process was not recorded for safe cleanup.' }
    $helperEntry | Add-Member -NotePropertyName TestInstallerPath -NotePropertyValue $installer
    $helperReady = Join-Path $jobRoot 'helper.ready'
    $handshakeDeadline = [DateTime]::UtcNow.AddSeconds(20)
    while (![IO.File]::Exists($helperReady) -and !$helper.HasExited -and [DateTime]::UtcNow -lt $handshakeDeadline) {
        Start-Sleep -Milliseconds 100
        $helper.Refresh()
    }
    if (![IO.File]::Exists($helperReady)) {
        $helperStatus = if ($helper.HasExited) { "exit=$($helper.ExitCode)" } else { 'still running' }
        throw "The update helper did not complete its 20-second ready handshake ($helperStatus)."
    }

    $parentEntry = $script:ownedProcesses | Where-Object { $_.ProcessId -eq $parent.Id } | Select-Object -First 1
    if ($null -eq $parentEntry -or !(Test-OwnedProcess $parentEntry)) {
        throw 'The background launcher PID, start time, or executable path no longer matches the process started by this test.'
    }
    if (!(Test-SamePath $parent.MainModule.FileName $targetExe)) { throw 'Refusing to stop a launcher outside the isolated target directory.' }
    $parent.Kill()
    if (!$parent.WaitForExit(10000)) { throw 'The test parent launcher did not exit after its verified PID was stopped.' }

    $helperDeadline = [DateTime]::UtcNow.AddSeconds(120)
    while (!$helper.HasExited -and [DateTime]::UtcNow -lt $helperDeadline) {
        Start-Sleep -Milliseconds 200
        $helper.Refresh()
    }
    if (!$helper.HasExited) { throw 'The update helper did not exit within 120 seconds.' }
    $helperExitCode = $helper.ExitCode
    $expectedExitCode = if ($ExpectSuccess) { 0 } else { 1 }
    if ($helperExitCode -ne $expectedExitCode) {
        throw "$Name helper exited with $helperExitCode; expected $expectedExitCode. See $jobRoot\install.log"
    }

    $restarted = Wait-TestRestart $targetExe $data 30
    $resultPath = Join-Path $data 'self-update-result.json'
    $lastResultPath = "$resultPath.last"
    $resultDeadline = [DateTime]::UtcNow.AddSeconds(20)
    $result = $null
    $selectedResult = $null
    while ($null -eq $result -and [DateTime]::UtcNow -lt $resultDeadline) {
        foreach ($candidateResult in @($resultPath, $lastResultPath)) {
            if (![IO.File]::Exists($candidateResult)) { continue }
            try {
                $result = Get-Content -LiteralPath $candidateResult -Raw | ConvertFrom-Json
                $selectedResult = $candidateResult
                break
            }
            catch { $result = $null }
        }
        if ($null -ne $result) { break }
        Start-Sleep -Milliseconds 150
    }
    if ($null -eq $result) { throw "$Name result JSON was not retained as self-update-result.json or .last." }
    if ([bool]$result.Success -ne $ExpectSuccess) { throw "$Name result Success was '$($result.Success)' instead of '$ExpectSuccess'." }
    if ($result.Version -ne $Version) { throw "$Name result version '$($result.Version)' did not match '$Version'." }

    if ($ExpectSuccess) {
        $actualVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo($targetExe).ProductVersion.Split('+')[0]
        if ($actualVersion -ne $Version) { throw "Success target has ProductVersion '$actualVersion', expected '$Version'." }
    }
    else {
        $launcherDllHashAfter = (Get-FileHash -LiteralPath $launcherDll -Algorithm SHA256).Hash
        if ($launcherDllHashAfter -ne $launcherDllHashBefore) {
            throw 'The failing installer changed MoaLauncher.dll despite rollback.'
        }
    }

    $settingsHashAfter = (Get-FileHash -LiteralPath $settingsPath -Algorithm SHA256).Hash
    $userFileHashAfter = (Get-FileHash -LiteralPath $userFile -Algorithm SHA256).Hash
    if ($settingsHashAfter -ne $settingsHashBefore) { throw "$Name settings.json changed during the update." }
    if ($userFileHashAfter -ne $userFileHashBefore) { throw "$Name unrelated user file changed during the update." }

    [pscustomobject]@{
        Case = $Name
        TargetDirectory = $target
        DataDirectory = $data
        Version = $result.Version
        HelperExitCode = $helperExitCode
        Success = [bool]$result.Success
        ResultPath = $selectedResult
        SettingsPreserved = $true
        UserFilePreserved = $true
        LauncherDllRolledBack = if ($ExpectSuccess) { $null } else { $true }
        RestartedProcessId = $restarted.ProcessId
    }
}

function Compile-FailingInstaller([string]$OutputPath, [string]$WorkDirectory) {
    $compilerCandidates = @(
        (Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'),
        (Join-Path $env:WINDIR 'Microsoft.NET/Framework/v4.0.30319/csc.exe')
    )
    $compiler = $compilerCandidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    if (!$compiler) { throw 'The Windows .NET Framework csc.exe compiler was not found for the isolated failure fixture.' }
    $source = Assert-UnderRoot $runRoot (Join-Path $WorkDirectory 'FailingInstaller.cs')
    $sourceText = @'
using System;
using System.IO;

internal static class FailingInstaller
{
    private static int Main(string[] args)
    {
        string target = null;
        foreach (string arg in args)
        {
            if (arg.StartsWith("/DIR=", StringComparison.OrdinalIgnoreCase))
                target = arg.Substring(5).Trim('"');
        }

        if (String.IsNullOrWhiteSpace(target)) return 40;
        string marker = Path.Combine(target, "self-update-validation-target.txt");
        if (!File.Exists(marker)) return 41;
        File.WriteAllText(Path.Combine(target, "MoaLauncher.dll"), "partial update from isolated failure fixture");
        return 42;
    }
}
'@
    [void](New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($OutputPath)) -Force)
    Set-Content -LiteralPath $source -Value $sourceText -Encoding utf8NoBOM
    $start = [Diagnostics.ProcessStartInfo]::new([IO.Path]::GetFullPath($compiler))
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.WorkingDirectory = $WorkDirectory
    foreach ($argument in @('/nologo', '/target:winexe', "/out:$OutputPath", $source)) { [void]$start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($start)
    if ($null -eq $process) { throw 'Could not start csc.exe for the failure fixture.' }
    try {
        if (!$process.WaitForExit(30000)) { $process.Kill(); throw 'csc.exe timed out compiling the failure fixture.' }
        if ($process.ExitCode -ne 0) { throw "csc.exe failed with exit code $($process.ExitCode)." }
    }
    finally { $process.Dispose() }
    if (!(Test-Path -LiteralPath $OutputPath -PathType Leaf)) { throw 'The failure fixture installer was not created.' }
}

function Stop-OwnedTestProcesses {
    foreach ($helperEntry in @($script:ownedProcesses | Where-Object { $_.PSObject.Properties.Name -contains 'TestInstallerPath' })) {
        try {
            if (!(Test-OwnedProcess $helperEntry)) { continue }
            $children = Get-CimInstance -ClassName Win32_Process -Filter "ParentProcessId=$($helperEntry.ProcessId)" -ErrorAction Stop
            foreach ($child in $children) {
                if ([string]::IsNullOrWhiteSpace($child.ExecutablePath) -or
                    !(Test-SamePath $child.ExecutablePath $helperEntry.TestInstallerPath)) { continue }
                try {
                    $installerProcess = [Diagnostics.Process]::GetProcessById([int]$child.ProcessId)
                    $installerProcess.Refresh()
                    if (!$installerProcess.HasExited -and (Test-SamePath $installerProcess.MainModule.FileName $helperEntry.TestInstallerPath)) {
                        $installerStart = $installerProcess.StartTime.ToUniversalTime().Ticks
                        $installerProcess.Kill()
                        [void]$installerProcess.WaitForExit(5000)
                        Write-Verbose "Stopped test installer child $($child.ProcessId) at $($helperEntry.TestInstallerPath), start ticks $installerStart."
                    }
                    $installerProcess.Dispose()
                }
                catch { }
            }
        }
        catch { }
    }

    foreach ($entry in @($script:ownedProcesses | Sort-Object -Property ProcessId -Descending)) {
        if (!(Test-OwnedProcess $entry)) { continue }
        try {
            $entry.Process.Kill()
            [void]$entry.Process.WaitForExit(5000)
        }
        catch { }
    }
    foreach ($entry in $script:ownedProcesses) {
        try { $entry.Process.Dispose() } catch { }
    }
}

$published = Get-FullInputPath $PublishedDirectory 'PublishedDirectory'
$installerInput = Get-FullInputPath $InstallerPath 'InstallerPath'
$baseline = if ($OptionalBaselineDirectory) { Get-FullInputPath $OptionalBaselineDirectory 'OptionalBaselineDirectory' } else { $published }
foreach ($directory in @($published, $baseline)) {
    if (!(Test-Path -LiteralPath $directory -PathType Container)) { throw "Input is not a directory: $directory" }
    $requiredFiles = if (Test-SamePath $directory $published) { @($launcherName, 'MoaLauncher.dll', 'launcher-files.json') } else { @($launcherName, 'MoaLauncher.dll') }
    foreach ($required in $requiredFiles) {
        if (!(Test-Path -LiteralPath (Join-Path $directory $required) -PathType Leaf)) { throw "Published launcher input is missing '$required': $directory" }
    }
}
if (!(Test-Path -LiteralPath $installerInput -PathType Leaf)) { throw "InstallerPath is not a file: $installerInput" }
$publishedVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo((Join-Path $published $launcherName)).ProductVersion.Split('+')[0]
$versionPattern = '^\d+\.\d+\.\d+$'
if ($publishedVersion -notmatch $versionPattern) { throw "PublishedDirectory has an unsupported ProductVersion '$publishedVersion'." }
if (![IO.Path]::GetFileName($installerInput).Equals("MoaLauncher-$publishedVersion-Setup-x64.exe", [StringComparison]::OrdinalIgnoreCase)) {
    throw "InstallerPath must be MoaLauncher-$publishedVersion-Setup-x64.exe to match PublishedDirectory."
}
$testVersion = $publishedVersion
if ($OptionalBaselineDirectory) {
    $baselineVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo((Join-Path $baseline $launcherName)).ProductVersion.Split('+')[0]
    if ($baselineVersion -notmatch $versionPattern -or [Version]$baselineVersion -ge [Version]$publishedVersion) {
        throw "OptionalBaselineDirectory must be older than PublishedDirectory ($publishedVersion); found '$baselineVersion'."
    }
}

$null = Assert-UnderRoot $artifactRoot $validationRoot
$null = Assert-UnderRoot $validationRoot $runRoot
[void](New-Item -ItemType Directory -Path $runRoot -Force)
$results = [Collections.Generic.List[object]]::new()
try {
    $successResult = Start-Case -Name 'success' -SourceTarget $baseline -InstallerSource $installerInput -Version $testVersion -ExpectSuccess $true
    $results.Add($successResult)

    $failureFixtureDirectory = Assert-UnderRoot $runRoot (Join-Path $runRoot 'failure-fixture')
    $failureInstaller = Assert-UnderRoot $runRoot (Join-Path $failureFixtureDirectory 'MoaLauncher-99.0.0-Setup-x64.exe')
    Compile-FailingInstaller $failureInstaller $failureFixtureDirectory
    $failureResult = Start-Case -Name 'failure' -SourceTarget $published -InstallerSource $failureInstaller -Version '99.0.0' -ExpectSuccess $false
    $results.Add($failureResult)

    $summaryPath = Assert-UnderRoot $runRoot (Join-Path $runRoot 'validation-results.json')
    $results | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $summaryPath -Encoding utf8NoBOM
    Write-Host "Self-update helper validation passed. Artifacts retained at: $runRoot"
    $results | Format-Table Case, Version, HelperExitCode, Success, SettingsPreserved, UserFilePreserved, LauncherDllRolledBack
}
finally {
    Stop-OwnedTestProcesses
}
