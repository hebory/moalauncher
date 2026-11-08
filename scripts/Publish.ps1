[CmdletBinding()]
param(
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,
    [ValidatePattern('^8\.0\.\d+$')]
    [string]$RuntimeVersion = '8.0.33',
    [string]$RestoreSource,
    [string]$IsccPath
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$propsPath = Join-Path $repositoryRoot 'Directory.Build.props'
[xml]$props = Get-Content -LiteralPath $propsPath
$projectVersion = [string]$props.Project.PropertyGroup.Version
if (!$Version) { $Version = $projectVersion }
if (!$Version -or $Version -ne $projectVersion) {
    throw "Publish version '$Version' must match Directory.Build.props version '$projectVersion'."
}
$artifactRoot = [IO.Path]::GetFullPath((Join-Path $repositoryRoot 'artifacts'))
$publishDirectory = [IO.Path]::GetFullPath((Join-Path $artifactRoot 'publish/win-x64'))
$distributionDirectory = [IO.Path]::GetFullPath((Join-Path $artifactRoot 'distribution'))

function Assert-InArtifactRoot([string]$Path) {
    $root = $artifactRoot.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    $resolved = [IO.Path]::GetFullPath($Path)
    if (!$resolved.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to modify a path outside artifacts/: $resolved"
    }
    return $resolved
}

$null = Assert-InArtifactRoot $publishDirectory
$null = Assert-InArtifactRoot $distributionDirectory

New-Item -ItemType Directory -Path $artifactRoot -Force | Out-Null
if (Test-Path -LiteralPath $publishDirectory) {
    Remove-Item -LiteralPath $publishDirectory -Recurse -Force
}
if (Test-Path -LiteralPath $distributionDirectory) {
    Remove-Item -LiteralPath $distributionDirectory -Recurse -Force
}
New-Item -ItemType Directory -Path $distributionDirectory -Force | Out-Null

$restoreArguments = @()
if ($RestoreSource) { $restoreArguments += "-p:RestoreSources=$RestoreSource" }
& dotnet publish (Join-Path $repositoryRoot 'src/MoaLauncher.App/MoaLauncher.App.csproj') `
    -c Release -r win-x64 --self-contained true `
    "-p:Version=$Version" "-p:RuntimeFrameworkVersion=$RuntimeVersion" `
    -p:PublishTrimmed=false -p:PublishReadyToRun=false -p:RestoreIgnoreFailedSources=false `
    -o $publishDirectory -m:1 -v:minimal @restoreArguments
if ($LASTEXITCODE -ne 0) { throw "Moa Launcher publish failed with exit code $LASTEXITCODE." }

$executable = Join-Path $publishDirectory 'MoaLauncher.exe'
if (!(Test-Path -LiteralPath $executable -PathType Leaf)) { throw 'Published MoaLauncher.exe was not found.' }
$actualVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo($executable).ProductVersion.Split('+')[0]
if ($actualVersion -ne $Version) { throw "Published version '$actualVersion' does not match requested version '$Version'." }

# Include the bundled .NET and Windows Desktop runtime license notices when present.
$nugetRoot = if ($env:NUGET_PACKAGES) { $env:NUGET_PACKAGES } else { Join-Path $env:USERPROFILE '.nuget/packages' }
$licenseDirectory = Join-Path $publishDirectory 'ThirdPartyNotices'
foreach ($packageName in @('microsoft.netcore.app.runtime.win-x64', 'microsoft.windowsdesktop.app.runtime.win-x64')) {
    $packageDirectory = Join-Path $nugetRoot "$packageName/$RuntimeVersion"
    if (!(Test-Path -LiteralPath $packageDirectory -PathType Container)) {
        throw "Runtime package license directory not found: $packageDirectory"
    }
    $copied = $false
    foreach ($noticeName in @('LICENSE.TXT', 'LICENSE', 'THIRD-PARTY-NOTICES.TXT')) {
        $notice = Join-Path $packageDirectory $noticeName
        if (Test-Path -LiteralPath $notice -PathType Leaf) {
            New-Item -ItemType Directory -Path $licenseDirectory -Force | Out-Null
            Copy-Item -LiteralPath $notice -Destination (Join-Path $licenseDirectory "$packageName-$noticeName")
            $copied = $true
        }
    }
    if (!$copied) { throw "Runtime package license notices are missing: $packageName" }
}

# The detached updater copies only these packaged files, preserving unrelated user files.
$manifestPath = Join-Path $publishDirectory 'launcher-files.json'
$packagedFiles = @(Get-ChildItem -LiteralPath $publishDirectory -File -Recurse | ForEach-Object {
    [IO.Path]::GetRelativePath($publishDirectory, $_.FullName).Replace('\', '/')
}) + @('launcher-files.json')
ConvertTo-Json -InputObject @($packagedFiles | Sort-Object -Unique) | Set-Content -LiteralPath $manifestPath -Encoding utf8

$archiveName = "moa-launcher-$Version-windows-x64.zip"
$archivePath = Join-Path $distributionDirectory $archiveName
Compress-Archive -Path (Join-Path $publishDirectory '*') -DestinationPath $archivePath -CompressionLevel Optimal
$hash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
"$hash  $archiveName" | Set-Content -LiteralPath "$archivePath.sha256" -Encoding ascii

if (!$IsccPath) {
    $candidatePaths = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6/ISCC.exe')
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) }
    if ($candidatePaths.Count -gt 0) { $IsccPath = $candidatePaths[0] }
}
if (!$IsccPath -or !(Test-Path -LiteralPath $IsccPath -PathType Leaf)) {
    throw 'Inno Setup Compiler (ISCC.exe) was not found. Install Inno Setup 6 or pass -IsccPath with a valid ISCC.exe path.'
}
$IsccPath = (Resolve-Path -LiteralPath $IsccPath).Path
$installerScript = Join-Path $repositoryRoot 'installer/MoaLauncher.iss'
& $IsccPath "/DVersion=$Version" "/DPublishDir=$publishDirectory" "/DOutputDir=$distributionDirectory" $installerScript
if ($LASTEXITCODE -ne 0) { throw "Inno Setup compilation failed with exit code $LASTEXITCODE." }

$installerName = "MoaLauncher-$Version-Setup-x64.exe"
$installerPath = Join-Path $distributionDirectory $installerName
if (!(Test-Path -LiteralPath $installerPath -PathType Leaf)) { throw "Installer was not created: $installerPath" }
$installerHash = (Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash.ToLowerInvariant()
"$installerHash  $installerName" | Set-Content -LiteralPath "$installerPath.sha256" -Encoding ascii
@("$hash *$archiveName", "$installerHash *$installerName") | Set-Content -LiteralPath (Join-Path $distributionDirectory 'SHA256SUMS.txt') -Encoding ascii

[pscustomobject]@{
    Version = $Version
    Runtime = $RuntimeVersion
    Archive = $archivePath
    Bytes = (Get-Item -LiteralPath $archivePath).Length
    SHA256 = $hash
    Installer = $installerPath
    InstallerSHA256 = $installerHash
} | Format-List
