[CmdletBinding()]
param(
    [string]$MakensisPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$validationRoot = [IO.Path]::GetFullPath((Join-Path $repositoryRoot 'artifacts/nsis-validation'))
$fixtureRoot = [IO.Path]::GetFullPath((Join-Path $validationRoot 'fixtures'))
$versionsRoot = [IO.Path]::GetFullPath((Join-Path $fixtureRoot 'versions'))
$installersRoot = [IO.Path]::GetFullPath((Join-Path $fixtureRoot 'installers'))

if ([string]::IsNullOrWhiteSpace($MakensisPath)) {
    $candidates = @(
        (Join-Path $env:ProgramFiles 'NSIS/makensis.exe'),
        (Join-Path ${env:ProgramFiles(x86)} 'NSIS/makensis.exe')
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) }
    $MakensisPath = $candidates | Select-Object -First 1
}

if ([string]::IsNullOrWhiteSpace($MakensisPath) -or !(Test-Path -LiteralPath $MakensisPath -PathType Leaf)) {
    throw 'makensis.exe를 찾을 수 없습니다. -MakensisPath로 NSIS 컴파일러 경로를 지정하세요.'
}
$MakensisPath = (Resolve-Path -LiteralPath $MakensisPath).Path

foreach ($directory in @($versionsRoot, $installersRoot)) {
    [void](New-Item -ItemType Directory -Path $directory -Force)
}

function Write-FixtureProject([string]$Version) {
    $projectDirectory = Join-Path $versionsRoot $Version
    $outputDirectory = Join-Path $projectDirectory 'out'
    [void](New-Item -ItemType Directory -Path $outputDirectory -Force)
    $projectPath = Join-Path $projectDirectory 'NsisFixture.csproj'
    $sourcePath = Join-Path $projectDirectory 'Program.cs'
    $project = @"
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net8.0</TargetFramework>
    <AssemblyName>NsisFixture</AssemblyName>
    <Version>$Version</Version>
    <AssemblyVersion>$Version.0</AssemblyVersion>
    <FileVersion>$Version.0</FileVersion>
    <InformationalVersion>$Version</InformationalVersion>
  </PropertyGroup>
</Project>
"@
    [IO.File]::WriteAllText($projectPath, $project, [Text.UTF8Encoding]::new($false))
    [IO.File]::WriteAllText($sourcePath, 'Console.WriteLine("NSIS validation fixture");', [Text.UTF8Encoding]::new($false))
    & dotnet build $projectPath --configuration Release --output $outputDirectory --nologo | Out-Host
    if ($LASTEXITCODE -ne 0) { throw ".NET fixture $Version 빌드에 실패했습니다." }
    $executable = Join-Path $outputDirectory 'NsisFixture.exe'
    if (!(Test-Path -LiteralPath $executable -PathType Leaf)) { throw ".NET fixture 실행 파일을 찾을 수 없습니다: $executable" }
    return $executable
}

function Quote-NsisPath([string]$Path) {
    if ($Path.Contains('"')) { throw "NSIS fixture 경로에 큰따옴표를 사용할 수 없습니다: $Path" }
    return '"' + $Path + '"'
}

function Compile-NsisInstaller([string]$Name, [string]$Executable, [string]$Mode = 'install') {
    $outputPath = Join-Path $installersRoot "$Name.exe"
    $scriptPath = Join-Path $installersRoot "$Name.nsi"
    $lines = @(
        'Unicode true',
        'RequestExecutionLevel user',
        'Name "MoaLauncher NSIS validation"',
        ('OutFile ' + (Quote-NsisPath $outputPath)),
        'InstallDir "$TEMP\MoaLauncherNsisValidation"',
        'SilentInstall silent'
    )
    if ($Mode -eq 'failure') {
        $lines += @('Function .onInit', '  SetErrorLevel 42', '  Quit', 'FunctionEnd', 'Section', 'SectionEnd')
    }
    elseif ($Mode -eq 'missing') {
        $lines += @('Section', '  SetOutPath "$INSTDIR"', ('  File /oname=fixture-marker.txt ' + (Quote-NsisPath $Executable)), 'SectionEnd')
    }
    else {
        $lines += @('Section', '  SetOutPath "$INSTDIR"', ('  File /oname=NsisFixture.exe ' + (Quote-NsisPath $Executable)), 'SectionEnd')
    }
    [IO.File]::WriteAllText($scriptPath, ($lines -join "`r`n"), [Text.UTF8Encoding]::new($false))
    & $MakensisPath '/V2' $scriptPath
    if ($LASTEXITCODE -ne 0) { throw "NSIS fixture '$Name' 컴파일에 실패했습니다." }
    if (!(Test-Path -LiteralPath $outputPath -PathType Leaf)) { throw "NSIS fixture 설치기를 찾을 수 없습니다: $outputPath" }
}

$fixture10 = Write-FixtureProject '1.0.0'
$fixture11 = Write-FixtureProject '1.1.0'
Compile-NsisInstaller 'setup-1.0.0' $fixture10
Compile-NsisInstaller 'setup-1.1.0' $fixture11
Compile-NsisInstaller 'setup-failure' $fixture11 'failure'
Compile-NsisInstaller 'setup-missing-executable' $fixture11 'missing'
Compile-NsisInstaller 'setup-version-mismatch' $fixture10

$previousFixtureRoot = $env:MOALAUNCHER_NSIS_FIXTURE_ROOT
try {
    $env:MOALAUNCHER_NSIS_FIXTURE_ROOT = $fixtureRoot
    & dotnet test (Join-Path $repositoryRoot 'tests/MoaLauncher.Core.Tests/MoaLauncher.Core.Tests.csproj') `
        --configuration Release --filter 'FullyQualifiedName~NsisIntegrationTests' --no-restore
    if ($LASTEXITCODE -ne 0) { throw 'NSIS 실제 설치 통합 검증에 실패했습니다.' }
}
finally {
    if ($null -eq $previousFixtureRoot) { Remove-Item Env:MOALAUNCHER_NSIS_FIXTURE_ROOT -ErrorAction SilentlyContinue }
    else { $env:MOALAUNCHER_NSIS_FIXTURE_ROOT = $previousFixtureRoot }
}

Write-Host "NSIS 통합 검증이 완료되었습니다. Fixture: $fixtureRoot"
