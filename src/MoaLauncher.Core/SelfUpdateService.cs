using System.Security.Cryptography;

namespace MoaLauncher.Core;

/// <summary>Stages a verified Moa Launcher installer without executing it.</summary>
public sealed class SelfUpdateService
{
    public const string Repository = "hebory/moalauncher";
    public const string InstallerPattern = "MoaLauncher-*-Setup-x64.exe";
    public const string IconUrl = "https://raw.githubusercontent.com/hebory/moalauncher/master/icon.png";
    public const string CoverUrl = "https://raw.githubusercontent.com/hebory/moalauncher/master/cover.png";

    public static AppDefinition Definition { get; } = new()
    {
        Id = "moa-launcher",
        Name = "Moa Launcher",
        Description = "모아 프로그램 업데이트 런처",
        Category = "런처",
        AccentColor = "#8BA3FF",
        Monogram = "M",
        Repository = Repository,
        IconUrl = IconUrl,
        CoverUrl = CoverUrl,
        Packages =
        [
            new PackageDefinition
            {
                Architecture = "x64",
                Kind = "inno",
                AssetPattern = InstallerPattern,
                ExecutablePath = "MoaLauncher.exe",
                InstallDirectory = "Programs/MoaLauncher"
            }
        ]
    };

    private readonly string stagingRoot;
    private readonly VerifiedDownloader downloader;
    private readonly InstallationService checksumService;

    public SelfUpdateService(HttpClient client, string dataDirectory)
    {
        ArgumentNullException.ThrowIfNull(client);
        var fullDataDirectory = Path.GetFullPath(dataDirectory ?? throw new ArgumentNullException(nameof(dataDirectory)));
        stagingRoot = Path.GetFullPath(Path.Combine(fullDataDirectory, "self-update"));
        downloader = new VerifiedDownloader(client);
        checksumService = new InstallationService(client, fullDataDirectory);
    }

    public async Task<PreparedSelfUpdate> PrepareAsync(AppRelease release, string currentVersion,
        IProgress<InstallProgress>? progress = null, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(release);
        ArgumentException.ThrowIfNullOrWhiteSpace(currentVersion);
        cancellationToken.ThrowIfCancellationRequested();

        if (!ReleaseRules.IsNewer(release.Tag, currentVersion))
            throw new InvalidOperationException("현재 버전보다 새로운 런처 릴리스가 아닙니다.");
        if (!IsOfficialReleasePage(release))
            throw new InvalidDataException("런처 릴리스 페이지가 공식 GitHub 저장소와 일치하지 않습니다.");

        var package = ReleaseRules.SelectPackage(Definition);
        var asset = ReleaseRules.SelectAsset(release, package);
        if (!asset.Name.EndsWith("-Setup-x64.exe", StringComparison.OrdinalIgnoreCase) ||
            !ReleaseRules.ValidateAssetUrl(Repository, asset.DownloadUrl))
            throw new InvalidDataException("런처 설치 파일이 공식 GitHub 저장소에 게시되지 않았습니다.");
        VerifiedDownloader.RequireAllowedReleaseUri(asset.DownloadUrl);

        var version = NormalizeVersion(release.Tag);
        Directory.CreateDirectory(stagingRoot);
        var jobId = Guid.NewGuid().ToString("N");
        var jobDirectory = Path.GetFullPath(Path.Combine(stagingRoot, jobId));
        if (!IsOwnedJobDirectory(jobDirectory, jobId))
            throw new InvalidOperationException("런처 업데이트 임시 폴더 경로가 올바르지 않습니다.");

        Directory.CreateDirectory(jobDirectory);
        var installerPath = SafePaths.ResolveRelative(jobDirectory, asset.Name);
        var success = false;
        try
        {
            progress?.Report(new InstallProgress("런처 업데이트 다운로드 준비", 0, asset.Size > 0 ? asset.Size : null));
            var result = await downloader.DownloadAsync(asset.DownloadUrl, installerPath,
                VerifiedDownloader.DefaultMaximumBytes, progress, cancellationToken).ConfigureAwait(false);
            var expectedSha256 = await checksumService.GetExpectedSha256Async(release, asset, Repository, cancellationToken).ConfigureAwait(false);
            var actualBytes = Convert.FromHexString(result.Sha256);
            var expectedBytes = Convert.FromHexString(expectedSha256);
            if (!CryptographicOperations.FixedTimeEquals(actualBytes, expectedBytes))
                throw new InvalidDataException("다운로드한 런처 설치 파일의 SHA-256 검증에 실패했습니다.");

            cancellationToken.ThrowIfCancellationRequested();
            progress?.Report(new InstallProgress("런처 업데이트 다운로드 완료", result.Bytes, result.Bytes));
            success = true;
            return new PreparedSelfUpdate(installerPath, version, result.Sha256);
        }
        finally
        {
            if (!success) DeleteOwnedJobDirectory(jobDirectory, jobId);
        }
    }

    private static string NormalizeVersion(string tag)
    {
        var version = tag.Trim();
        if (version.StartsWith('v') || version.StartsWith('V')) version = version[1..];
        var buildMetadataIndex = version.IndexOf('+');
        if (buildMetadataIndex >= 0) version = version[..buildMetadataIndex];
        return version;
    }

    private static bool IsOfficialReleasePage(AppRelease release)
    {
        var uri = release.PageUrl;
        if (!uri.IsAbsoluteUri || !string.Equals(uri.Scheme, Uri.UriSchemeHttps, StringComparison.OrdinalIgnoreCase) ||
            !string.Equals(uri.IdnHost, "github.com", StringComparison.OrdinalIgnoreCase) || !uri.IsDefaultPort ||
            uri.UserInfo.Length != 0 || uri.Query.Length != 0 || uri.Fragment.Length != 0)
            return false;

        var parts = uri.AbsolutePath.Split('/', StringSplitOptions.RemoveEmptyEntries).Select(Uri.UnescapeDataString).ToArray();
        return parts.Length == 5 &&
               string.Equals(parts[0], "hebory", StringComparison.OrdinalIgnoreCase) &&
               string.Equals(parts[1], "moalauncher", StringComparison.OrdinalIgnoreCase) &&
               parts[2] == "releases" && parts[3] == "tag" &&
               string.Equals(parts[4], release.Tag, StringComparison.Ordinal);
    }

    private bool IsOwnedJobDirectory(string jobDirectory, string jobId)
    {
        if (!Guid.TryParseExact(jobId, "N", out _)) return false;
        var fullPath = Path.GetFullPath(jobDirectory);
        return SafePaths.IsWithin(stagingRoot, fullPath) &&
               string.Equals(Path.GetDirectoryName(fullPath), stagingRoot, StringComparison.OrdinalIgnoreCase) &&
               string.Equals(Path.GetFileName(fullPath), jobId, StringComparison.OrdinalIgnoreCase);
    }

    private void DeleteOwnedJobDirectory(string jobDirectory, string jobId)
    {
        if (!IsOwnedJobDirectory(jobDirectory, jobId) || !Directory.Exists(jobDirectory)) return;
        try { Directory.Delete(jobDirectory, recursive: true); }
        catch (IOException) { }
        catch (UnauthorizedAccessException) { }
    }
}

public sealed record PreparedSelfUpdate(string InstallerPath, string Version, string Sha256);
