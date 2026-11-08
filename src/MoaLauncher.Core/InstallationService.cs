using System.Diagnostics;
using System.IO.Compression;
using System.Security.Cryptography;
using System.Text.Json;

namespace MoaLauncher.Core;

public sealed class InstallationService : IInstallationService
{
    private const long MaxPackageBytes = 2L * 1024 * 1024 * 1024;
    private const long MaxExpandedBytes = 5L * 1024 * 1024 * 1024;
    private const long MaxEntryBytes = 2L * 1024 * 1024 * 1024;
    private const int MaxZipEntries = 100_000;
    private readonly string _dataDirectory;
    private readonly string _localAppData;
    private readonly VerifiedDownloader _downloader;
    private static readonly JsonSerializerOptions StateJsonOptions = new(CatalogService.JsonOptions)
    {
        WriteIndented = true
    };

    public InstallationService(HttpClient client, string dataDirectory, string? localAppData = null)
    {
        ArgumentNullException.ThrowIfNull(client);
        _dataDirectory = Path.GetFullPath(dataDirectory ?? throw new ArgumentNullException(nameof(dataDirectory)));
        _localAppData = Path.GetFullPath(localAppData ?? Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData));
        _downloader = new VerifiedDownloader(client);
    }

    public async Task<InstalledApp?> FindInstalledAsync(AppDefinition app, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(app);
        cancellationToken.ThrowIfCancellationRequested();
        var package = ReleaseRules.SelectPackage(app);
        var expectedDirectory = GetInstallDirectory(app, package, null);
        var expectedExecutable = SafePaths.ResolveRelative(expectedDirectory, package.ExecutablePath);
        var statePath = GetStatePath(app);

        if (package.Kind.Equals("inno", StringComparison.OrdinalIgnoreCase) && File.Exists(expectedExecutable))
        {
            var actualVersion = ReadProductVersion(expectedExecutable);
            if (actualVersion is null) return null;
            if (File.Exists(statePath))
            {
                var previous = await ReadStateAsync(statePath, cancellationToken).ConfigureAwait(false);
                if (previous is not null && previous.AppId == app.Id && previous.InstallDirectory is not null
                    && previous.ExecutablePath is not null && PathEquals(previous.InstallDirectory, expectedDirectory)
                    && PathEquals(previous.ExecutablePath, expectedExecutable))
                    return previous with { Version = actualVersion };
            }
            return new InstalledApp(app.Id, actualVersion, expectedExecutable, expectedDirectory, package.Kind, File.GetLastWriteTimeUtc(expectedExecutable));
        }

        if (File.Exists(statePath))
        {
            var stored = await ReadStateAsync(statePath, cancellationToken).ConfigureAwait(false);
            if (stored is not null && stored.AppId == app.Id
                && stored.InstallDirectory is not null && stored.ExecutablePath is not null
                && PathEquals(stored.InstallDirectory, expectedDirectory)
                && PathEquals(stored.ExecutablePath, expectedExecutable)
                && File.Exists(expectedExecutable))
                return stored;
        }

        return null;
    }

    public async Task<InstalledApp> InstallAsync(AppDefinition app, AppRelease release,
        IProgress<InstallProgress>? progress = null, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(app);
        ArgumentNullException.ThrowIfNull(release);
        cancellationToken.ThrowIfCancellationRequested();
        var package = ReleaseRules.SelectPackage(app);
        var asset = ReleaseRules.SelectAsset(release, package);
        if (!ReleaseRules.ValidateAssetUrl(app.Repository, asset.DownloadUrl))
            throw new InvalidDataException("릴리스 파일이 지정된 GitHub 저장소에서 오지 않았습니다.");
        VerifiedDownloader.RequireAllowedReleaseUri(asset.DownloadUrl);
        var previousInstall = await FindInstalledAsync(app, cancellationToken).ConfigureAwait(false);
        if (previousInstall is not null && IsRunning(previousInstall))
            throw new InvalidOperationException("프로그램이 실행 중이므로 파일을 교체하지 않았습니다. 앱을 닫은 뒤 다시 시도하세요.");

        var downloadRoot = Path.Combine(_dataDirectory, "downloads", SafePaths.RequireSegment(app.Id));
        Directory.CreateDirectory(downloadRoot);
        var downloadPath = Path.Combine(downloadRoot, Guid.NewGuid().ToString("N") + Path.GetExtension(asset.Name));
        try
        {
            progress?.Report(new InstallProgress("다운로드 준비", 0, asset.Size > 0 ? asset.Size : null));
            var result = await _downloader.DownloadAsync(asset.DownloadUrl, downloadPath, MaxPackageBytes, progress, cancellationToken).ConfigureAwait(false);
            var expectedHash = await GetExpectedSha256Async(release, asset, app.Repository, cancellationToken).ConfigureAwait(false);
            if (!CryptographicOperations.FixedTimeEquals(Convert.FromHexString(result.Sha256), Convert.FromHexString(expectedHash)))
                throw new InvalidDataException("다운로드한 파일의 SHA-256 검증에 실패했습니다.");

            cancellationToken.ThrowIfCancellationRequested();
            if (package.Kind.Equals("zip", StringComparison.OrdinalIgnoreCase))
                return await InstallZipAsync(app, release, package, downloadPath, progress, cancellationToken).ConfigureAwait(false);
            if (package.Kind.Equals("inno", StringComparison.OrdinalIgnoreCase))
                return await InstallInnoAsync(app, release, package, downloadPath, progress, cancellationToken).ConfigureAwait(false);
            throw new NotSupportedException($"지원하지 않는 설치 패키지입니다: {package.Kind}");
        }
        finally
        {
            if (File.Exists(downloadPath)) File.Delete(downloadPath);
        }
    }

    public bool IsRunning(InstalledApp app)
    {
        ArgumentNullException.ThrowIfNull(app);
        var expected = Path.GetFullPath(app.ExecutablePath);
        foreach (var process in Process.GetProcesses())
        {
            using (process)
            {
                try
                {
                    if (process.MainModule?.FileName is { } filename && PathEquals(filename, expected)) return true;
                }
                catch (Exception ex) when (ex is InvalidOperationException or System.ComponentModel.Win32Exception or NotSupportedException)
                {
                    try
                    {
                        if (Path.GetFileNameWithoutExtension(process.ProcessName).Equals(
                                Path.GetFileNameWithoutExtension(expected), StringComparison.OrdinalIgnoreCase)) return true;
                    }
                    catch (Exception identityError) when (identityError is InvalidOperationException or System.ComponentModel.Win32Exception)
                    {
                        // The process exited while its protected identity was being checked.
                    }
                }
            }
        }
        return false;
    }

    public void Launch(InstalledApp app)
    {
        ArgumentNullException.ThrowIfNull(app);
        var executable = Path.GetFullPath(app.ExecutablePath);
        var directory = Path.GetFullPath(app.InstallDirectory);
        if (!SafePaths.IsWithin(directory, executable) || !File.Exists(executable))
            throw new FileNotFoundException("설치된 프로그램 실행 파일을 찾을 수 없습니다.", executable);
        Process.Start(new ProcessStartInfo
        {
            FileName = executable,
            WorkingDirectory = directory,
            UseShellExecute = false
        });
    }

    private async Task<InstalledApp> InstallZipAsync(AppDefinition app, AppRelease release, PackageDefinition package,
        string archivePath, IProgress<InstallProgress>? progress, CancellationToken cancellationToken)
    {
        var version = SafePaths.RequireSegment(release.Tag.TrimStart('v', 'V'), nameof(release));
        var installRoot = Path.Combine(_dataDirectory, "apps", SafePaths.RequireSegment(app.Id), version);
        var appsRoot = Path.Combine(_dataDirectory, "apps", SafePaths.RequireSegment(app.Id));
        Directory.CreateDirectory(appsRoot);
        var stage = Path.Combine(appsRoot, ".staging-" + Guid.NewGuid().ToString("N"));
        var committedDirectory = false;
        try
        {
            progress?.Report(new InstallProgress("압축 파일 검사"));
            Directory.CreateDirectory(stage);
            await ExtractZipSafelyAsync(archivePath, stage, progress, cancellationToken).ConfigureAwait(false);
            var executable = SafePaths.ResolveRelative(stage, package.ExecutablePath);
            if (!File.Exists(executable)) throw new InvalidDataException("패키지에 지정된 실행 파일이 없습니다.");
            cancellationToken.ThrowIfCancellationRequested();
            if (Directory.Exists(installRoot))
                throw new IOException("같은 버전의 설치 폴더가 이미 있습니다. 기존 파일을 보존하기 위해 설치를 중단했습니다.");
            Directory.Move(stage, installRoot);
            committedDirectory = true;
            var installed = new InstalledApp(app.Id, release.Tag, SafePaths.ResolveRelative(installRoot, package.ExecutablePath),
                installRoot, package.Kind, DateTimeOffset.UtcNow);
            await SaveStateAsync(app, installed, cancellationToken).ConfigureAwait(false);
            committedDirectory = false;
            progress?.Report(new InstallProgress("설치 완료", 1, 1));
            return installed;
        }
        finally
        {
            if (Directory.Exists(stage)) Directory.Delete(stage, recursive: true);
            if (committedDirectory && Directory.Exists(installRoot)) Directory.Delete(installRoot, recursive: true);
        }
    }

    private async Task<InstalledApp> InstallInnoAsync(AppDefinition app, AppRelease release, PackageDefinition package,
        string installerPath, IProgress<InstallProgress>? progress, CancellationToken cancellationToken)
    {
        var directory = GetInstallDirectory(app, package, null);
        var executable = SafePaths.ResolveRelative(directory, package.ExecutablePath);
        Directory.CreateDirectory(directory);
        var existing = new InstalledApp(app.Id, ReadProductVersion(executable) ?? "", executable, directory, package.Kind, DateTimeOffset.UtcNow);
        if (File.Exists(executable) && IsRunning(existing))
            throw new InvalidOperationException("모아뷰가 실행 중이므로 파일을 교체하지 않았습니다. 앱을 닫은 뒤 다시 시도하세요.");

        progress?.Report(new InstallProgress("설치 프로그램 실행"));
        var start = new ProcessStartInfo
        {
            FileName = installerPath,
            WorkingDirectory = Path.GetDirectoryName(installerPath)!,
            UseShellExecute = false
        };
        foreach (var argument in new[] { "/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/NOCLOSEAPPLICATIONS", "/NORESTARTAPPLICATIONS", "/SP-", $"/DIR={directory}" })
            start.ArgumentList.Add(argument);
        using var process = Process.Start(start) ?? throw new InvalidOperationException("설치 프로그램을 시작하지 못했습니다.");
        progress?.Report(new InstallProgress("설치 중 · 설치 프로그램 창에서 취소할 수 있습니다"));
        await process.WaitForExitAsync(CancellationToken.None).ConfigureAwait(false);
        if (process.ExitCode is not (0 or 3010))
            throw new InvalidOperationException($"설치 프로그램이 오류 코드 {process.ExitCode}(으)로 종료되었습니다. 설치 프로그램이 변경한 파일은 자동으로 되돌릴 수 없습니다.");
        if (!File.Exists(executable)) throw new InvalidDataException("설치 후 실행 파일을 찾을 수 없습니다. 설치 프로그램이 변경한 파일은 자동으로 되돌릴 수 없습니다.");
        var actualVersion = ReadProductVersion(executable);
        var expectedVersion = release.Tag.TrimStart('v', 'V');
        if (actualVersion is null || !actualVersion.Equals(expectedVersion, StringComparison.OrdinalIgnoreCase))
            throw new InvalidDataException($"설치된 프로그램 버전({actualVersion ?? "확인 불가"})이 릴리스 버전({expectedVersion})과 다릅니다. 설치 프로그램이 변경한 파일은 자동으로 되돌릴 수 없습니다.");

        var installed = new InstalledApp(app.Id, actualVersion, executable, directory, package.Kind, DateTimeOffset.UtcNow);
        try
        {
            await SaveStateAsync(app, installed, CancellationToken.None).ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            throw new IOException("프로그램 설치는 끝났지만 런처 상태를 저장하지 못했습니다. 설치 프로그램이 변경한 파일은 자동으로 되돌릴 수 없습니다.", ex);
        }
        progress?.Report(new InstallProgress("설치 완료", 1, 1));
        return installed;
    }

    private static async Task ExtractZipSafelyAsync(string archivePath, string stage, IProgress<InstallProgress>? progress, CancellationToken cancellationToken)
    {
        await using var archiveStream = new FileStream(archivePath, FileMode.Open, FileAccess.Read, FileShare.Read, 81920, FileOptions.Asynchronous | FileOptions.SequentialScan);
        using var archive = new ZipArchive(archiveStream, ZipArchiveMode.Read, leaveOpen: false);
        if (archive.Entries.Count > MaxZipEntries) throw new InvalidDataException("ZIP 파일 항목 수가 허용 범위를 초과했습니다.");
        var names = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        long expanded = 0;
        var buffer = new byte[81920];
        foreach (var entry in archive.Entries)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var directoryEntry = entry.FullName.EndsWith('/') || entry.FullName.EndsWith('\\');
            var relative = directoryEntry ? entry.FullName.TrimEnd('/', '\\') : entry.FullName;
            if (relative.Length == 0) continue;
            var destination = SafePaths.ResolveRelative(stage, relative);
            if (!names.Add(destination)) throw new InvalidDataException("ZIP 파일에 중복 경로가 있습니다.");
            var unixType = (entry.ExternalAttributes >> 16) & 0xF000;
            if (unixType == 0xA000 || (entry.ExternalAttributes & 0x400) != 0)
                throw new InvalidDataException("ZIP 심볼릭 링크나 재분석 지점은 허용되지 않습니다.");
            if (directoryEntry)
            {
                Directory.CreateDirectory(destination);
                continue;
            }
            if (entry.Length < 0 || entry.Length > MaxEntryBytes || entry.CompressedLength < 0)
                throw new InvalidDataException("ZIP 항목 크기가 허용 범위를 초과했습니다.");
            expanded = checked(expanded + entry.Length);
            if (expanded > MaxExpandedBytes) throw new InvalidDataException("ZIP 압축 해제 크기가 허용 범위를 초과했습니다.");
            if (entry.Length > 1_048_576 && entry.CompressedLength == 0)
                throw new InvalidDataException("ZIP 압축 비율이 허용 범위를 초과했습니다.");
            if (entry.CompressedLength > 0 && entry.Length / Math.Max(1, entry.CompressedLength) > 500)
                throw new InvalidDataException("ZIP 압축 비율이 허용 범위를 초과했습니다.");

            Directory.CreateDirectory(Path.GetDirectoryName(destination)!);
            await using var input = entry.Open();
            await using var output = new FileStream(destination, FileMode.CreateNew, FileAccess.Write, FileShare.None, 81920, FileOptions.Asynchronous | FileOptions.SequentialScan);
            long copied = 0;
            int read;
            while ((read = await input.ReadAsync(buffer, cancellationToken).ConfigureAwait(false)) != 0)
            {
                copied = checked(copied + read);
                if (copied > entry.Length || copied > MaxEntryBytes) throw new InvalidDataException("ZIP 항목이 선언된 크기를 초과했습니다.");
                await output.WriteAsync(buffer.AsMemory(0, read), cancellationToken).ConfigureAwait(false);
            }
            if (copied != entry.Length) throw new InvalidDataException("ZIP 항목의 실제 크기가 선언된 크기와 다릅니다.");
            progress?.Report(new InstallProgress("압축 해제", expanded, MaxExpandedBytes));
        }
    }

    internal async Task<string> GetExpectedSha256Async(AppRelease release, ReleaseAsset asset, string repository, CancellationToken cancellationToken)
    {
        if (asset.Sha256 is { } direct && IsSha256(direct)) return direct.ToUpperInvariant();
        var checksumAsset = release.Assets.FirstOrDefault(a => a.Name.Equals(asset.Name + ".sha256", StringComparison.OrdinalIgnoreCase));
        if (checksumAsset is not null)
        {
            if (!ReleaseRules.ValidateAssetUrl(repository, checksumAsset.DownloadUrl))
                throw new InvalidDataException("체크섬 파일이 지정된 GitHub 저장소에서 오지 않았습니다.");
            VerifiedDownloader.RequireAllowedReleaseUri(checksumAsset.DownloadUrl);
            var text = await DownloadSmallTextAsync(checksumAsset.DownloadUrl, cancellationToken).ConfigureAwait(false);
            if (VerifiedDownloader.TryParseChecksum(text, asset.Name, out var parsed) && parsed is not null) return parsed;
            var bare = text.Trim();
            if (IsSha256(bare)) return bare.ToUpperInvariant();
        }

        checksumAsset = release.Assets.FirstOrDefault(a => a.Name.Equals("SHA256SUMS.txt", StringComparison.OrdinalIgnoreCase));
        if (checksumAsset is not null)
        {
            if (!ReleaseRules.ValidateAssetUrl(repository, checksumAsset.DownloadUrl))
                throw new InvalidDataException("체크섬 파일이 지정된 GitHub 저장소에서 오지 않았습니다.");
            VerifiedDownloader.RequireAllowedReleaseUri(checksumAsset.DownloadUrl);
            var text = await DownloadSmallTextAsync(checksumAsset.DownloadUrl, cancellationToken).ConfigureAwait(false);
            if (VerifiedDownloader.TryParseChecksum(text, asset.Name, out var parsed) && parsed is not null) return parsed;
        }
        throw new InvalidDataException("릴리스에 이 파일과 일치하는 SHA-256 체크섬이 없어 설치를 중단했습니다.");
    }

    private async Task<string> DownloadSmallTextAsync(Uri uri, CancellationToken cancellationToken)
    {
        var path = Path.Combine(_dataDirectory, "downloads", Guid.NewGuid().ToString("N") + ".checksum");
        try
        {
            await _downloader.DownloadAsync(uri, path, 1024 * 1024, cancellationToken: cancellationToken).ConfigureAwait(false);
            return await File.ReadAllTextAsync(path, cancellationToken).ConfigureAwait(false);
        }
        finally
        {
            if (File.Exists(path)) File.Delete(path);
        }
    }

    private async Task SaveStateAsync(AppDefinition app, InstalledApp installed, CancellationToken cancellationToken)
    {
        var path = GetStatePath(app);
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        var temporary = path + ".tmp-" + Guid.NewGuid().ToString("N");
        try
        {
            await using (var stream = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write, FileShare.None, 4096, FileOptions.Asynchronous | FileOptions.WriteThrough))
            {
                await JsonSerializer.SerializeAsync(stream, installed, StateJsonOptions, cancellationToken).ConfigureAwait(false);
                await stream.FlushAsync(cancellationToken).ConfigureAwait(false);
            }
            cancellationToken.ThrowIfCancellationRequested();
            File.Move(temporary, path, overwrite: true);
        }
        finally
        {
            if (File.Exists(temporary)) File.Delete(temporary);
        }
    }

    private async Task<InstalledApp?> ReadStateAsync(string path, CancellationToken cancellationToken)
    {
        await using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read, 4096, FileOptions.Asynchronous | FileOptions.SequentialScan);
        return await JsonSerializer.DeserializeAsync<InstalledApp>(stream, StateJsonOptions, cancellationToken).ConfigureAwait(false);
    }

    private string GetStatePath(AppDefinition app) => Path.Combine(_dataDirectory, "installed", SafePaths.RequireSegment(app.Id) + ".json");

    private string GetInstallDirectory(AppDefinition app, PackageDefinition package, string? version)
    {
        if (package.Kind.Equals("inno", StringComparison.OrdinalIgnoreCase))
        {
            if (string.IsNullOrWhiteSpace(package.InstallDirectory)) throw new InvalidDataException("Inno Setup 패키지에 설치 폴더가 지정되지 않았습니다.");
            return SafePaths.ResolveRelative(_localAppData, package.InstallDirectory);
        }
        if (version is null)
        {
            var statePath = GetStatePath(app);
            if (!File.Exists(statePath)) return Path.Combine(_dataDirectory, "apps", SafePaths.RequireSegment(app.Id), "unknown");
            var stored = JsonSerializer.Deserialize<InstalledApp>(File.ReadAllText(statePath), StateJsonOptions);
            version = stored?.Version?.TrimStart('v', 'V') ?? "unknown";
        }
        return Path.Combine(_dataDirectory, "apps", SafePaths.RequireSegment(app.Id), SafePaths.RequireSegment(version));
    }

    private static string? ReadProductVersion(string executable)
    {
        try
        {
            var value = FileVersionInfo.GetVersionInfo(executable).ProductVersion?.Split('+')[0];
            if (string.IsNullOrWhiteSpace(value)) return null;
            var parts = value.Split('.');
            if (parts.Length == 4 && parts[3] == "0") value = string.Join('.', parts[..3]);
            return value;
        }
        catch (Exception ex) when (ex is IOException or ArgumentException or UnauthorizedAccessException)
        {
            return null;
        }
    }

    private static bool IsSha256(string value) => value.Length == 64 && value.All(Uri.IsHexDigit);

    private static bool PathEquals(string first, string second) => Path.GetFullPath(first).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar)
        .Equals(Path.GetFullPath(second).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar), StringComparison.OrdinalIgnoreCase);
}
