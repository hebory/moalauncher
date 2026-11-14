[CmdletBinding()]
param(
    [string]$MakensisPath,
    [string]$IsccPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$validationRoot = [IO.Path]::GetFullPath((Join-Path $repositoryRoot 'artifacts/rust-install-validation'))
$fixtureRoot = [IO.Path]::GetFullPath((Join-Path $validationRoot ("run-" + [Guid]::NewGuid().ToString('N'))))
$versionsRoot = Join-Path $fixtureRoot 'versions'
$installersRoot = Join-Path $fixtureRoot 'installers'

function Find-Tool([string]$ExplicitPath, [string[]]$Candidates, [string]$Name) {
    if (![string]::IsNullOrWhiteSpace($ExplicitPath)) {
        if (!(Test-Path -LiteralPath $ExplicitPath -PathType Leaf)) { throw "$Name was not found: $ExplicitPath" }
        return (Resolve-Path -LiteralPath $ExplicitPath).Path
    }
    $found = $Candidates | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) } | Select-Object -First 1
    if (!$found) { throw "$Name was not found. Pass its path explicitly." }
    return (Resolve-Path -LiteralPath $found).Path
}

$MakensisPath = Find-Tool $MakensisPath @(
    (Join-Path $env:LOCALAPPDATA 'tauri/NSIS/makensis.exe'),
    (Join-Path $env:ProgramFiles 'NSIS/makensis.exe'),
    (Join-Path ${env:ProgramFiles(x86)} 'NSIS/makensis.exe')
) 'makensis.exe'
$IsccPath = Find-Tool $IsccPath @(
    (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe'),
    (Join-Path $env:ProgramFiles 'Inno Setup 6/ISCC.exe')
) 'Inno Setup Compiler'

foreach ($directory in @($versionsRoot, $installersRoot)) {
    [void](New-Item -ItemType Directory -Path $directory -Force)
}

function Write-FixtureProject([string]$Version) {
    $projectDirectory = Join-Path $versionsRoot $Version
    [void](New-Item -ItemType Directory -Path $projectDirectory -Force)
    $projectPath = Join-Path $projectDirectory 'MoaFixture.csproj'
    $sourcePath = Join-Path $projectDirectory 'Program.cs'
    $outputDirectory = Join-Path $projectDirectory 'out'
    $project = @"
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net8.0</TargetFramework>
    <AssemblyName>MoaFixture</AssemblyName>
    <Version>$Version</Version>
    <AssemblyVersion>$Version.0</AssemblyVersion>
    <FileVersion>$Version.0</FileVersion>
    <InformationalVersion>$Version</InformationalVersion>
  </PropertyGroup>
</Project>
"@
    [IO.File]::WriteAllText($projectPath, $project, [Text.UTF8Encoding]::new($false))
    [IO.File]::WriteAllText($sourcePath, 'Console.WriteLine("Moa Launcher Rust installer fixture");', [Text.UTF8Encoding]::new($false))
    & dotnet build $projectPath --configuration Release --output $outputDirectory --nologo | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "MoaFixture $Version build failed." }
    $executable = Join-Path $outputDirectory 'MoaFixture.exe'
    if (!(Test-Path -LiteralPath $executable -PathType Leaf)) { throw "MoaFixture executable is missing: $executable" }
    return $executable
}

function Quote-InstallerPath([string]$Path) {
    if ($Path.Contains('"')) { throw "Fixture path contains a quote: $Path" }
    return '"' + $Path + '"'
}

function Compile-NsisFixture([string]$Version, [string]$Executable, [bool]$Failure = $false) {
    $name = if ($Failure) { "setup-nsis-failure" } else { "setup-nsis-$Version" }
    $scriptPath = Join-Path $installersRoot "$name.nsi"
    $outputPath = Join-Path $installersRoot "$name.exe"
    $lines = @(
        'Unicode true',
        'RequestExecutionLevel user',
        'Name "Moa Launcher Rust validation fixture"',
        ('OutFile ' + (Quote-InstallerPath $outputPath)),
        'InstallDir "$TEMP\MoaLauncherRustFixture"',
        'SilentInstall silent'
    )
    if ($Failure) {
        $lines += @('Function .onInit', '  SetErrorLevel 42', '  Quit', 'FunctionEnd', 'Section', 'SectionEnd')
    }
    else {
        $lines += @('Section', '  SetOutPath "$INSTDIR"', ('  File /oname=MoaFixture.exe ' + (Quote-InstallerPath $Executable)), 'SectionEnd')
    }
    [IO.File]::WriteAllText($scriptPath, ($lines -join "`r`n"), [Text.UTF8Encoding]::new($false))
    & $MakensisPath '/V2' $scriptPath
    if ($LASTEXITCODE -ne 0) { throw "NSIS fixture $name compile failed." }
}

function Compile-InnoFixture([string]$Version, [string]$Executable, [bool]$Failure = $false) {
    $name = if ($Failure) { "setup-inno-failure" } else { "setup-inno-$Version" }
    $scriptPath = Join-Path $installersRoot "$name.iss"
    $lines = @(
        '[Setup]',
        'AppId={{E179EF56-CB26-4FC7-9C7D-93C6B2E710AA}',
        'AppName=Moa Launcher Rust validation fixture',
        'AppVersion=1.0.0',
        'DefaultDirName={tmp}\MoaLauncherRustFixture',
        'PrivilegesRequired=lowest',
        'Uninstallable=no',
        'DisableProgramGroupPage=yes',
        'DisableDirPage=yes',
        'DisableReadyPage=yes',
        'DisableFinishedPage=yes',
        ('OutputDir=' + (Quote-InstallerPath $installersRoot)),
        ('OutputBaseFilename=' + $name),
        'Compression=none',
        'SolidCompression=no'
    )
    if ($Failure) {
        $lines += @('[Code]', 'function InitializeSetup(): Boolean;', 'begin', '  Result := False;', 'end;')
    }
    else {
        $lines += @('[Files]', ('Source: ' + (Quote-InstallerPath $Executable) + '; DestDir: "{app}"; DestName: "MoaFixture.exe"; Flags: ignoreversion'))
    }
    [IO.File]::WriteAllText($scriptPath, ($lines -join "`r`n"), [Text.UTF8Encoding]::new($false))
    & $IsccPath '/Qp' $scriptPath
    if ($LASTEXITCODE -ne 0) { throw "Inno fixture $name compile failed." }
}

$fixture10 = Write-FixtureProject '1.0.0'
$fixture11 = Write-FixtureProject '1.1.0'
foreach ($kind in @('nsis', 'inno')) {
    if ($kind -eq 'nsis') {
        Compile-NsisFixture '1.0.0' $fixture10
        Compile-NsisFixture '1.1.0' $fixture11
        Compile-NsisFixture '1.1.0' $fixture11 $true
    }
    else {
        Compile-InnoFixture '1.0.0' $fixture10
        Compile-InnoFixture '1.1.0' $fixture11
        Compile-InnoFixture '1.1.0' $fixture11 $true
    }
}

$previous = $env:MOALAUNCHER_RUST_INSTALL_FIXTURE_ROOT
try {
    $env:MOALAUNCHER_RUST_INSTALL_FIXTURE_ROOT = $fixtureRoot
    & cargo test --manifest-path (Join-Path $repositoryRoot 'src-tauri/Cargo.toml') installer_fixtures_install_update_and_preserve_state_after_failure -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'Rust NSIS/Inno installation and update fixture tests failed.' }
}
finally {
    if ($null -eq $previous) { Remove-Item Env:MOALAUNCHER_RUST_INSTALL_FIXTURE_ROOT -ErrorAction SilentlyContinue }
    else { $env:MOALAUNCHER_RUST_INSTALL_FIXTURE_ROOT = $previous }
}

Write-Host "Rust NSIS/Inno fixture verification completed under $fixtureRoot"
