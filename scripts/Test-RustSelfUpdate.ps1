[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$runnerSource = Join-Path $repositoryRoot 'src-tauri/target/release/MoaLauncher.exe'
if (!(Test-Path -LiteralPath $runnerSource -PathType Leaf)) {
    $runnerSource = Join-Path $repositoryRoot 'src-tauri/target/debug/MoaLauncher.exe'
}
if (!(Test-Path -LiteralPath $runnerSource -PathType Leaf)) {
    throw "Rust helper runner is not built. Expected release or debug executable under src-tauri/target."
}

$artifactRoot = [IO.Path]::GetFullPath((Join-Path $repositoryRoot 'artifacts'))
$validationRoot = [IO.Path]::GetFullPath((Join-Path $artifactRoot 'self-update-validation'))
$artifactPrefix = $artifactRoot.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if (!$validationRoot.StartsWith($artifactPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to write outside artifacts/: $validationRoot"
}
$runRoot = Join-Path $validationRoot ("rust-helper-" + [guid]::NewGuid().ToString('N'))
$sourceRoot = Join-Path $runRoot 'sources'
$payloadRoot = Join-Path $runRoot 'payloads'
$script:fixtureOutputCaptures = @{}
New-Item -ItemType Directory -Path $sourceRoot, $payloadRoot -Force | Out-Null

$compilerCandidates = @(
    (Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'),
    (Join-Path $env:WINDIR 'Microsoft.NET/Framework/v4.0.30319/csc.exe')
)
$cscPath = $compilerCandidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
if (!$cscPath) { throw 'Microsoft .NET Framework csc.exe was not found for isolated fixture executables.' }

$launcherSource = @'
using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;
using System.Reflection;
using System.Runtime.Versioning;

[assembly: AssemblyVersion("__VERSION__")]
[assembly: AssemblyFileVersion("__VERSION__")]
[assembly: AssemblyInformationalVersion("__PRODUCT_VERSION__")]

internal static class FakeLauncher
{
    [StructLayout(LayoutKind.Sequential)] private struct FILETIME { public uint low; public uint high; }
    [DllImport("kernel32.dll")] private static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll", SetLastError=true)] private static extern bool GetProcessTimes(IntPtr process, out FILETIME created, out FILETIME exited, out FILETIME kernel, out FILETIME user);

    private static int Main(string[] args)
    {
        if (args.Length >= 2 && args[0] == "--hold")
        {
            string data = Path.GetFullPath(args[1]);
            FILETIME created, exited, kernel, user;
            if (!GetProcessTimes(GetCurrentProcess(), out created, out exited, out kernel, out user)) return 90;
            ulong started = ((ulong)created.high << 32) | created.low;
            string exe = Path.GetFullPath(Process.GetCurrentProcess().MainModule.FileName);
            File.WriteAllText(Path.Combine(data, "parent.json"), Process.GetCurrentProcess().Id + "\n" + started + "\n" + exe);
            while (!File.Exists(Path.Combine(data, "allow-parent-exit"))) Thread.Sleep(50);
            return 0;
        }

        string dataDir = null;
        for (int i = 0; i + 1 < args.Length; i++)
            if (args[i] == "--data-dir") dataDir = args[i + 1];
        if (dataDir != null)
        {
            dataDir = Path.GetFullPath(dataDir);
            Directory.CreateDirectory(dataDir);
            File.WriteAllText(Path.Combine(dataDir, "fake-restart.marker"), "restart invoked");
        }
        return 0;
    }
}
'@

function Compile-FakeLauncher([string]$Version, [string]$OutputPath) {
    $productVersion = [string]::Join('.', $Version.Split('.')[0..2])
    $source = $launcherSource.Replace('__VERSION__', $Version).Replace('__PRODUCT_VERSION__', $productVersion)
    $sourcePath = Join-Path $sourceRoot ("FakeLauncher-$($Version.Replace('.', '-')).cs")
    Set-Content -LiteralPath $sourcePath -Value $source -Encoding utf8
    & $cscPath /nologo /target:exe "/out:$OutputPath" $sourcePath
    if ($LASTEXITCODE -ne 0 -or !(Test-Path -LiteralPath $OutputPath -PathType Leaf)) {
        throw "Failed to compile isolated launcher fixture $Version."
    }
    $reported = [Diagnostics.FileVersionInfo]::GetVersionInfo($OutputPath).ProductVersion
    if (!$reported -or $reported -notmatch "^$([regex]::Escape($Version.Split('.')[0..2] -join '.'))(?:\.0)?(?:\+.*)?$") {
        throw "Fixture version resource is not readable as $Version (reported '$reported')."
    }
}

$oldLauncher = Join-Path $payloadRoot 'MoaLauncher-0.3.0.exe'
$newLauncher = Join-Path $payloadRoot 'MoaLauncher-0.3.1.exe'
$wrongLauncher = Join-Path $payloadRoot 'MoaLauncher-0.3.2.exe'
Compile-FakeLauncher '0.3.0.0' $oldLauncher
Compile-FakeLauncher '0.3.1.0' $newLauncher
Compile-FakeLauncher '0.3.2.0' $wrongLauncher

$installerSource = @'
using System;
using System.IO;

internal static class FakeInstaller
{
    private static int Main(string[] args)
    {
        string dirArg = null;
        string logArg = null;
        foreach (string arg in args)
        {
            if (arg.StartsWith("/DIR=", StringComparison.OrdinalIgnoreCase)) dirArg = arg;
            if (arg.StartsWith("/LOG=", StringComparison.OrdinalIgnoreCase)) logArg = arg;
        }
        if (dirArg == null) return 81;
        string target = dirArg.Substring(5).Trim('"');
        string log = logArg == null ? null : logArg.Substring(5).Trim('"');
        if (log != null) File.WriteAllText(log, "fixture installer invoked");
        string modeFile = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "installer-mode.txt");
        string mode = File.ReadAllText(modeFile).Trim();
        string source = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, mode == "WRONG_VERSION" ? "wrong.exe" : "new.exe");
        File.Copy(source, Path.Combine(target, "MoaLauncher.exe"), true);
        File.WriteAllText(Path.Combine(target, "installer-touched.txt"), mode);
        // Exercise rollback after a failing installer has already modified target files.
        if (mode == "FAIL") return 17;
        return 0;
    }
}
'@
$installerSourcePath = Join-Path $sourceRoot 'FakeInstaller.cs'
Set-Content -LiteralPath $installerSourcePath -Value $installerSource -Encoding utf8
$installerTemplate = Join-Path $payloadRoot 'FakeInstaller.exe'
& $cscPath /nologo /target:exe "/out:$installerTemplate" $installerSourcePath
if ($LASTEXITCODE -ne 0 -or !(Test-Path -LiteralPath $installerTemplate -PathType Leaf)) { throw 'Failed to compile isolated fake installer.' }

function Get-TreeDigest([string]$Directory) {
    $lines = @(Get-ChildItem -LiteralPath $Directory -File -Recurse | Sort-Object { [IO.Path]::GetRelativePath($Directory, $_.FullName) } | ForEach-Object {
        $relative = [IO.Path]::GetRelativePath($Directory, $_.FullName).Replace('\', '/')
        $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        "$relative`0$hash"
    })
    $bytes = [Text.Encoding]::UTF8.GetBytes(($lines -join "`n"))
    $digest = [Security.Cryptography.SHA256]::HashData($bytes)
    [Convert]::ToHexString($digest).ToLowerInvariant()
}

function Start-FixtureProcess([string]$Path, [string[]]$Arguments, [string]$CapturePrefix = $null) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Path
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    if ($CapturePrefix) {
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
    }
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($start)
    if ($CapturePrefix) {
        $script:fixtureOutputCaptures[$process.Id] = @{
            Stdout = $process.StandardOutput.ReadToEndAsync()
            Stderr = $process.StandardError.ReadToEndAsync()
            Prefix = $CapturePrefix
        }
    }
    return $process
}

function Save-FixtureProcessOutput([Diagnostics.Process]$Process) {
    if (!$Process -or !$script:fixtureOutputCaptures.ContainsKey($Process.Id)) { return }
    $capture = $script:fixtureOutputCaptures[$Process.Id]
    if (!$Process.HasExited) { $Process.WaitForExit() }
    $stdout = $capture.Stdout.GetAwaiter().GetResult()
    $stderr = $capture.Stderr.GetAwaiter().GetResult()
    [IO.File]::WriteAllText("$($capture.Prefix).stdout.log", $stdout, [Text.UTF8Encoding]::new($false))
    [IO.File]::WriteAllText("$($capture.Prefix).stderr.log", $stderr, [Text.UTF8Encoding]::new($false))
    $script:fixtureOutputCaptures.Remove($Process.Id) | Out-Null
}

function Wait-File([string]$Path, [Diagnostics.Process]$Process, [int]$Seconds, [string]$FailureMessage) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Test-Path -LiteralPath $Path -PathType Leaf) { return }
        if ($Process.HasExited) { throw "$FailureMessage (process exit $($Process.ExitCode))." }
        Start-Sleep -Milliseconds 100
    }
    throw "$FailureMessage (timed out)."
}

function Invoke-HelperCase([string]$Name, [ValidateSet('SUCCESS', 'FAIL', 'WRONG_VERSION')][string]$Mode) {
    $caseRoot = Join-Path $runRoot $Name
    $target = Join-Path $caseRoot 'target'
    $data = Join-Path $caseRoot 'data'
    $selfUpdate = Join-Path $data 'self-update'
    $job = Join-Path $selfUpdate ([guid]::NewGuid().ToString('N'))
    $runner = Join-Path $job 'runner'
    New-Item -ItemType Directory -Path $target, $data, $runner -Force | Out-Null
    Copy-Item -LiteralPath $oldLauncher -Destination (Join-Path $target 'MoaLauncher.exe')
    Set-Content -LiteralPath (Join-Path $target 'existing-launcher-state.json') -Value '{"installedVersion":"0.3.0"}' -Encoding ascii
    Set-Content -LiteralPath (Join-Path $data 'existing-user-state.json') -Value '{"marker":"must survive"}' -Encoding ascii
    Set-Content -LiteralPath (Join-Path $data 'installed-app-state.json') -Value '{"appId":"fixture-app","version":"5.4.3"}' -Encoding ascii
    $beforeTargetHash = Get-TreeDigest $target
    $beforeUserState = (Get-FileHash -LiteralPath (Join-Path $data 'existing-user-state.json') -Algorithm SHA256).Hash
    $beforeInstalledState = (Get-FileHash -LiteralPath (Join-Path $data 'installed-app-state.json') -Algorithm SHA256).Hash

    $installerName = 'MoaLauncher-0.3.1-Setup-x64.exe'
    $installer = Join-Path $job $installerName
    Copy-Item -LiteralPath $installerTemplate -Destination $installer
    Copy-Item -LiteralPath $newLauncher -Destination (Join-Path $job 'new.exe')
    Copy-Item -LiteralPath $wrongLauncher -Destination (Join-Path $job 'wrong.exe')
    Set-Content -LiteralPath (Join-Path $job 'installer-mode.txt') -Value $Mode -Encoding ascii
    $installerHash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()

    $parent = Start-FixtureProcess (Join-Path $target 'MoaLauncher.exe') @('--hold', $data)
    $helper = $null
    try {
        $parentMetaPath = Join-Path $data 'parent.json'
        Wait-File $parentMetaPath $parent 15 'Fake launcher parent failed to start'
        $metaText = Get-Content -LiteralPath $parentMetaPath -Raw -Encoding utf8
        $meta = $metaText.Split([string[]]@("`r`n", "`n"), [StringSplitOptions]::RemoveEmptyEntries)
        $parentPid = [uint32]$meta[0].Trim()
        $parentStarted = [uint64]$meta[1].Trim()
        $parentExe = [IO.Path]::GetFullPath($meta[2].Trim())
        if ($parentPid -ne $parent.Id -or $parentExe -ne [IO.Path]::GetFullPath((Join-Path $target 'MoaLauncher.exe'))) {
            throw 'Fixture parent metadata does not match the live fake executable process.'
        }

        $plan = [ordered]@{
            target = [IO.Path]::GetFullPath($target)
            data = [IO.Path]::GetFullPath($data)
            installer = [IO.Path]::GetFullPath($installer)
            sha256 = $installerHash
            version = '0.3.1'
            parentPid = $parentPid
            parentStarted = $parentStarted
            parentExe = $parentExe
            background = $false
        }
        $planPath = Join-Path $job 'plan.json'
        $planJson = $plan | ConvertTo-Json -Depth 4
        [IO.File]::WriteAllText($planPath, $planJson, [Text.UTF8Encoding]::new($false))
        Copy-Item -LiteralPath $runnerSource -Destination (Join-Path $runner 'MoaLauncher.exe')

        $helper = Start-FixtureProcess (Join-Path $runner 'MoaLauncher.exe') @('--apply-self-update', [IO.Path]::GetFullPath($planPath)) (Join-Path $caseRoot 'helper')
        try {
            Wait-File (Join-Path $job 'helper.ready') $helper 25 'Rust helper did not validate its plan and signal readiness'
        } catch {
            if ($helper.HasExited) { Save-FixtureProcessOutput $helper }
            throw
        }
        Set-Content -LiteralPath (Join-Path $data 'allow-parent-exit') -Value 'exit' -Encoding ascii

        if (!$parent.WaitForExit(10000)) { throw 'Fake launcher parent did not exit after its fixture signal.' }
        if (!$helper.WaitForExit(75000)) { throw 'Rust helper did not finish the isolated update within 75 seconds.' }
        Save-FixtureProcessOutput $helper

        $resultPath = Join-Path $data 'self-update-result.json'
        Wait-File $resultPath $helper 5 'Rust helper did not write its result file'
        $result = Get-Content -LiteralPath $resultPath -Raw | ConvertFrom-Json
        Wait-File (Join-Path $data 'fake-restart.marker') $helper 15 'Launcher restart marker was not written'

        $userStateAfter = (Get-FileHash -LiteralPath (Join-Path $data 'existing-user-state.json') -Algorithm SHA256).Hash
        $installedStateAfter = (Get-FileHash -LiteralPath (Join-Path $data 'installed-app-state.json') -Algorithm SHA256).Hash
        if ($userStateAfter -ne $beforeUserState -or $installedStateAfter -ne $beforeInstalledState) {
            throw "$Name changed fixture user or installed-app state."
        }

        $actualExe = Join-Path $target 'MoaLauncher.exe'
        $actualVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo($actualExe).ProductVersion
        $actualTreeHash = Get-TreeDigest $target
        if ($Mode -eq 'SUCCESS') {
            if (!$result.success -or $helper.ExitCode -ne 0) { throw 'Successful fixture update did not report success.' }
            if (!$actualVersion.StartsWith('0.3.1', [StringComparison]::Ordinal)) { throw "Updated fake launcher has unexpected version '$actualVersion'." }
            if (!(Test-Path -LiteralPath (Join-Path $target 'installer-touched.txt') -PathType Leaf)) { throw 'Fake installer changes were not retained after success.' }
        } else {
            if ($result.success -or $helper.ExitCode -eq 0) { throw "$Name unexpectedly succeeded." }
            if ($actualTreeHash -ne $beforeTargetHash) { throw "$Name did not restore all prior launcher files and state." }
            if (!$actualVersion.StartsWith('0.3.0', [StringComparison]::Ordinal)) { throw "$Name did not restore the previous launcher version: $actualVersion" }
        }

        [pscustomobject]@{
            Case = $Name
            InstallerMode = $Mode
            HelperExitCode = $helper.ExitCode
            ReportedSuccess = [bool]$result.success
            LauncherVersionAfter = $actualVersion
            LauncherFilesPreserved = if ($Mode -eq 'SUCCESS') { $true } else { $actualTreeHash -eq $beforeTargetHash }
            ExistingDataStatePreserved = $userStateAfter -eq $beforeUserState -and $installedStateAfter -eq $beforeInstalledState
            ParentPid = $parentPid
            ParentStartFileTime = $parentStarted
        }
    } finally {
        if ($helper -and !$helper.HasExited) { $helper.Kill($true); $helper.WaitForExit() }
        if ($parent -and !$parent.HasExited) { $parent.Kill($true); $parent.WaitForExit() }
    }
}

$results = @(
    Invoke-HelperCase 'success' 'SUCCESS'
    Invoke-HelperCase 'installer-failure' 'FAIL'
    Invoke-HelperCase 'version-mismatch' 'WRONG_VERSION'
)
$results | Format-Table -AutoSize
$results | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $runRoot 'report.json') -Encoding utf8
Write-Host "Fixture root: $runRoot"
