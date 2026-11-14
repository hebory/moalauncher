[CmdletBinding()]
param(
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,
    [string]$IsccPath
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$cargoPath = Join-Path $repositoryRoot 'src-tauri/Cargo.toml'
$tauriConfigPath = Join-Path $repositoryRoot 'src-tauri/tauri.conf.json'
$npmPath = Join-Path $repositoryRoot 'package.json'
$cargoText = Get-Content -LiteralPath $cargoPath -Raw
if ($cargoText -notmatch '(?ms)^\[package\]\s*.*?^version\s*=\s*"(?<version>\d+\.\d+\.\d+)"') {
    throw 'src-tauri/Cargo.toml package version is missing or invalid.'
}
$cargoVersion = $Matches.version
$tauriVersion = [string](Get-Content -LiteralPath $tauriConfigPath -Raw | ConvertFrom-Json).version
$npmVersion = [string](Get-Content -LiteralPath $npmPath -Raw | ConvertFrom-Json).version
if (!$Version) { $Version = $cargoVersion }
if ($Version -ne $cargoVersion -or $Version -ne $tauriVersion -or $Version -ne $npmVersion) {
    throw "Requested version '$Version' must match Cargo ($cargoVersion), Tauri ($tauriVersion), and npm ($npmVersion)."
}

$artifactRoot = [IO.Path]::GetFullPath((Join-Path $repositoryRoot 'artifacts'))
$distributionDirectory = [IO.Path]::GetFullPath((Join-Path $artifactRoot 'rust-distribution'))
$stageDirectory = Join-Path $distributionDirectory 'payload'
$artifactPrefix = $artifactRoot.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if (!$distributionDirectory.StartsWith($artifactPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to modify a path outside artifacts/: $distributionDirectory"
}
if (Test-Path -LiteralPath $distributionDirectory) { Remove-Item -LiteralPath $distributionDirectory -Recurse -Force }
New-Item -ItemType Directory -Path $stageDirectory -Force | Out-Null

$executable = Join-Path $repositoryRoot 'src-tauri/target/release/MoaLauncher.exe'
if (!(Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "Rust launcher executable was not found. Build src-tauri first: $executable"
}
$exeVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo($executable).ProductVersion
if ($exeVersion) {
    $exeVersion = $exeVersion.Split('+')[0]
    if ($exeVersion -notmatch '^(\d+\.\d+\.\d+)') { throw "Built executable version '$exeVersion' is not valid." }
    if ($Matches[1] -ne $Version) { throw "Built executable version '$exeVersion' does not match requested version '$Version'." }
}
Copy-Item -LiteralPath $executable -Destination (Join-Path $stageDirectory 'MoaLauncher.exe')
Copy-Item -LiteralPath (Join-Path $repositoryRoot 'LICENSE') -Destination (Join-Path $stageDirectory 'LICENSE')
New-Item -ItemType Directory -Path (Join-Path $stageDirectory 'catalog') -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $repositoryRoot 'catalog/apps.json') -Destination (Join-Path $stageDirectory 'catalog/apps.json') -Force

function Get-LicenseFiles([string]$Directory, [string]$DeclaredFile) {
    $candidates = @()
    if ($DeclaredFile) {
        $candidates += Join-Path $Directory $DeclaredFile
    }
    $candidates += Get-ChildItem -LiteralPath $Directory -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)(\.|$|-)'} |
        Sort-Object Name | Select-Object -ExpandProperty FullName
    return @($candidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -Unique)
}

$notices = [Collections.Generic.List[string]]::new()
$notices.Add("Moa Launcher $Version third-party license notices")
$notices.Add('')
$notices.Add('The launcher is distributed under the MIT License in LICENSE.')
$notices.Add('')

# Include notices and license texts for the installed npm dependency tree.
$npmPackages = & npm ls --all --parseable
if ($LASTEXITCODE -ne 0) { throw 'npm ls failed while collecting dependency license notices.' }
foreach ($packageDirectory in $npmPackages) {
    if (!$packageDirectory -or $packageDirectory -eq $repositoryRoot) { continue }
    $manifestPath = Join-Path $packageDirectory 'package.json'
    if (!(Test-Path -LiteralPath $manifestPath -PathType Leaf)) { continue }
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    $name = [string]$manifest.name
    $version = [string]$manifest.version
    $license = if ($manifest.license) { [string]$manifest.license } else { 'license not declared in package metadata' }
    $notices.Add("=" * 78)
    $notices.Add("npm: $name@$version | License: $license")
    $licenseFile = if ($manifest.licenseFile) { [string]$manifest.licenseFile } else { $null }
    $files = Get-LicenseFiles $packageDirectory $licenseFile
    if ($files.Count -eq 0) { $notices.Add('(No license text file was shipped in this npm package.)') }
    foreach ($file in $files) {
        $notices.Add("--- $([IO.Path]::GetFileName($file)) ---")
        $notices.Add((Get-Content -LiteralPath $file -Raw))
    }
}

# Include license texts for the resolved Cargo dependency graph.
$cargoMetadata = & cargo metadata --locked --format-version 1 --manifest-path $cargoPath | ConvertFrom-Json -AsHashtable
if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed while collecting dependency license notices.' }
$rootPackageId = $cargoMetadata.resolve.root
foreach ($package in $cargoMetadata.packages) {
    if ($package.id -eq $rootPackageId) { continue }
    $manifest = [string]$package.manifest_path
    $packageDirectory = Split-Path -Parent $manifest
    $license = if ($package.license) { [string]$package.license } else { 'license not declared in package metadata' }
    $notices.Add("=" * 78)
    $notices.Add("Cargo: $($package.name)@$($package.version) | License: $license")
    $licenseFile = if ($package.license_file) {
        [IO.Path]::GetRelativePath($packageDirectory, [string]$package.license_file)
    } else { $null }
    $files = Get-LicenseFiles $packageDirectory $licenseFile
    if ($files.Count -eq 0) { $notices.Add('(No license text file was shipped in this Cargo package.)') }
    foreach ($file in $files) {
        $notices.Add("--- $([IO.Path]::GetFileName($file)) ---")
        $notices.Add((Get-Content -LiteralPath $file -Raw))
    }
}
$notices | Set-Content -LiteralPath (Join-Path $stageDirectory 'ThirdPartyNotices.txt') -Encoding utf8

$manifestPath = Join-Path $stageDirectory 'launcher-files.json'
$packagedFiles = @(Get-ChildItem -LiteralPath $stageDirectory -File -Recurse | ForEach-Object {
    [IO.Path]::GetRelativePath($stageDirectory, $_.FullName).Replace('\', '/')
}) + @('launcher-files.json')
ConvertTo-Json -InputObject @($packagedFiles | Sort-Object -Unique) | Set-Content -LiteralPath $manifestPath -Encoding utf8

$archiveName = "moa-launcher-$Version-windows-x64.zip"
$archivePath = Join-Path $distributionDirectory $archiveName
Compress-Archive -Path (Join-Path $stageDirectory '*') -DestinationPath $archivePath -CompressionLevel Optimal
$archiveHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
"$archiveHash  $archiveName" | Set-Content -LiteralPath "$archivePath.sha256" -Encoding ascii

if (!$IsccPath) {
    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6/ISCC.exe')
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) }
    if ($candidates.Count -gt 0) { $IsccPath = $candidates[0] }
}
if (!$IsccPath -or !(Test-Path -LiteralPath $IsccPath -PathType Leaf)) {
    throw 'Inno Setup Compiler (ISCC.exe) was not found. Install Inno Setup 6 or pass -IsccPath with a valid path.'
}
$installerScript = Join-Path $repositoryRoot 'installer/MoaLauncher.iss'
& $IsccPath "/DVersion=$Version" "/DPublishDir=$stageDirectory" "/DOutputDir=$distributionDirectory" $installerScript
if ($LASTEXITCODE -ne 0) { throw "Inno Setup compilation failed with exit code $LASTEXITCODE." }
$installerName = "MoaLauncher-$Version-Setup-x64.exe"
$installerPath = Join-Path $distributionDirectory $installerName
if (!(Test-Path -LiteralPath $installerPath -PathType Leaf)) { throw "Installer was not created: $installerPath" }
$installerHash = (Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash.ToLowerInvariant()
"$installerHash  $installerName" | Set-Content -LiteralPath "$installerPath.sha256" -Encoding ascii
@("$archiveHash *$archiveName", "$installerHash *$installerName") |
    Set-Content -LiteralPath (Join-Path $distributionDirectory 'SHA256SUMS.txt') -Encoding ascii
Remove-Item -LiteralPath $stageDirectory -Recurse -Force

[pscustomobject]@{
    Version = $Version
    Executable = $executable
    Archive = $archivePath
    ArchiveSHA256 = $archiveHash
    Installer = $installerPath
    InstallerSHA256 = $installerHash
    Notices = 'ThirdPartyNotices.txt is included inside the ZIP and installer.'
} | Format-List
