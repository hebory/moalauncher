using System.Security.Cryptography;
using System.Net.Http.Headers;

namespace MoaLauncher.Core;

/// <summary>Downloads bounded HTTPS GitHub release files and verifies their SHA-256.</summary>
public sealed class VerifiedDownloader(HttpClient client, IGitHubTokenProvider? tokenProvider = null)
{
    public const long DefaultMaximumBytes = 2L * 1024 * 1024 * 1024;
    private static readonly TimeSpan DownloadTimeout = TimeSpan.FromMinutes(10);
    private static readonly HashSet<string> AllowedHosts = new(StringComparer.OrdinalIgnoreCase)
    {
        "github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com",
        "github-releases.githubusercontent.com"
    };

    private static bool IsRedirect(System.Net.HttpStatusCode status) => status is
        System.Net.HttpStatusCode.MovedPermanently or System.Net.HttpStatusCode.Redirect or
        System.Net.HttpStatusCode.SeeOther or System.Net.HttpStatusCode.TemporaryRedirect or
        System.Net.HttpStatusCode.PermanentRedirect;

    public static bool IsAllowedReleaseUri(Uri uri) => uri.IsAbsoluteUri && uri.Scheme == Uri.UriSchemeHttps
        && uri.UserInfo.Length == 0 && AllowedHosts.Contains(uri.IdnHost);

    public static void RequireAllowedReleaseUri(Uri uri)
    {
        if (!IsAllowedReleaseUri(uri)) throw new InvalidDataException("GitHub 릴리스의 HTTPS 다운로드 주소만 허용됩니다.");
    }

    public async Task<(long Bytes, string Sha256)> DownloadAsync(Uri uri, string destinationPath,
        long maximumBytes = DefaultMaximumBytes, IProgress<InstallProgress>? progress = null,
        CancellationToken cancellationToken = default)
    {
        RequireAllowedReleaseUri(uri);
        if (maximumBytes <= 0) throw new ArgumentOutOfRangeException(nameof(maximumBytes));
        Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(destinationPath))!);
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeout.CancelAfter(DownloadTimeout);
        var tempPath = destinationPath + ".download-" + Guid.NewGuid().ToString("N");
        try
        {
            var current = uri;
            HttpResponseMessage? response = null;
            for (var hop = 0; hop <= 5; hop++)
            {
                using var request = new HttpRequestMessage(HttpMethod.Get, current);
                response = await client.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, timeout.Token).ConfigureAwait(false);
                if (IsRedirect(response.StatusCode))
                {
                    var location = response.Headers.Location;
                    response.Dispose();
                    response = null;
                    if (hop == 5 || location is null)
                        throw new InvalidDataException("GitHub 다운로드 리디렉션 횟수 또는 주소가 올바르지 않습니다.");
                    current = location.IsAbsoluteUri ? location : new Uri(current, location);
                    RequireAllowedReleaseUri(current);
                    continue;
                }
                if (response.RequestMessage?.RequestUri is { } requestUri && requestUri != current)
                {
                    response.Dispose();
                    response = null;
                    throw new InvalidDataException("HTTP 클라이언트가 검사되지 않은 다운로드 리디렉션을 따랐습니다.");
                }
                break;
            }

            using (response ?? throw new InvalidDataException("GitHub 다운로드 응답이 비어 있습니다."))
            {
                response.EnsureSuccessStatusCode();
                return await SaveResponseAsync(response, destinationPath, tempPath, maximumBytes, progress, timeout.Token).ConfigureAwait(false);
            }
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested && timeout.IsCancellationRequested)
        {
            throw new TimeoutException("다운로드 시간이 초과되었습니다.");
        }
        finally
        {
            if (File.Exists(tempPath)) File.Delete(tempPath);
        }
    }

    public async Task<(long Bytes, string Sha256)> DownloadAssetAsync(ReleaseAsset asset, string repository,
        bool isPrivate, string destinationPath, long maximumBytes = DefaultMaximumBytes,
        IProgress<InstallProgress>? progress = null, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(asset);
        if (!isPrivate)
        {
            if (!ReleaseRules.ValidateAssetUrl(repository, asset.DownloadUrl))
                throw new InvalidDataException("릴리스 파일이 지정된 GitHub 저장소에서 오지 않았습니다.");
            return await DownloadAsync(asset.DownloadUrl, destinationPath, maximumBytes, progress, cancellationToken).ConfigureAwait(false);
        }

        if (!ReleaseRules.ValidatePrivateAssetApiUrl(repository, asset.ApiUrl))
            throw new InvalidDataException("비공개 릴리스 파일 API 주소가 지정된 저장소와 일치하지 않습니다.");
        if (maximumBytes <= 0) throw new ArgumentOutOfRangeException(nameof(maximumBytes));
        var token = RequireToken();
        var apiUrl = asset.ApiUrl!;
        Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(destinationPath))!);
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeout.CancelAfter(DownloadTimeout);
        var tempPath = destinationPath + ".download-" + Guid.NewGuid().ToString("N");
        try
        {
            using var request = new HttpRequestMessage(HttpMethod.Get, apiUrl);
            request.Headers.Accept.Add(new MediaTypeWithQualityHeaderValue("application/octet-stream"));
            request.Headers.UserAgent.ParseAdd("MoaLauncher/1.0");
            request.Headers.TryAddWithoutValidation("X-GitHub-Api-Version", "2022-11-28");
            try
            {
                request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", token);
            }
            catch (FormatException)
            {
                throw new InvalidOperationException("GitHub 인증 정보 형식이 올바르지 않습니다. 저장한 인증 정보를 다시 확인하세요.");
            }
            using var response = await client.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, timeout.Token).ConfigureAwait(false);
            if (response.RequestMessage?.RequestUri is not { } finalUri || finalUri != apiUrl)
                throw new InvalidDataException("HTTP 클라이언트가 인증된 자산 API 요청을 리디렉션했습니다. 자동 리디렉션을 꺼 주세요.");
            if (response.StatusCode is System.Net.HttpStatusCode.Unauthorized or System.Net.HttpStatusCode.Forbidden or System.Net.HttpStatusCode.NotFound)
                throw GitHubReleaseService.PrivateAccessError(response.StatusCode);
            if (IsRedirect(response.StatusCode))
            {
                var location = response.Headers.Location;
                if (location is null) throw new InvalidDataException("GitHub 비공개 자산의 다운로드 주소가 없습니다.");
                var downloadUrl = location.IsAbsoluteUri ? location : new Uri(apiUrl, location);
                RequireAllowedReleaseUri(downloadUrl);
                return await DownloadAsync(downloadUrl, destinationPath, maximumBytes, progress, timeout.Token).ConfigureAwait(false);
            }
            response.EnsureSuccessStatusCode();
            return await SaveResponseAsync(response, destinationPath, tempPath, maximumBytes, progress, timeout.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested && timeout.IsCancellationRequested)
        {
            throw new TimeoutException("다운로드 시간이 초과되었습니다.");
        }
        catch (HttpRequestException)
        {
            throw new InvalidOperationException("비공개 GitHub 릴리스 파일을 안전하게 다운로드하지 못했습니다. 네트워크 연결과 저장소 권한을 확인하세요.");
        }
        catch (IOException)
        {
            throw new InvalidOperationException("비공개 GitHub 릴리스 파일을 안전하게 저장하지 못했습니다. 디스크 공간과 권한을 확인하세요.");
        }
        finally
        {
            if (File.Exists(tempPath)) File.Delete(tempPath);
        }
    }

    private string RequireToken()
    {
        string? token;
        try { token = tokenProvider?.GetToken()?.Trim(); }
        catch (Exception ex) when (ex is not OperationCanceledException)
        {
            throw new InvalidOperationException("Windows 자격 증명 저장소에서 GitHub 인증 정보를 읽지 못했습니다.");
        }
        if (string.IsNullOrWhiteSpace(token) || token.Length > 4096 || token.Any(character =>
                char.IsControl(character) || char.IsWhiteSpace(character) || character < 0x21 || character > 0x7E))
            throw new InvalidOperationException("비공개 저장소를 다운로드하려면 GitHub 인증을 설정하세요.");
        return token;
    }

    private static async Task<(long Bytes, string Sha256)> SaveResponseAsync(HttpResponseMessage response,
        string destinationPath, string tempPath, long maximumBytes, IProgress<InstallProgress>? progress,
        CancellationToken cancellationToken)
    {
        if (response.Content.Headers.ContentLength is long length && length > maximumBytes)
            throw new InvalidDataException("다운로드 파일이 허용 크기를 초과했습니다.");
        await using var input = await response.Content.ReadAsStreamAsync(cancellationToken).ConfigureAwait(false);
        using var hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        var buffer = new byte[81920];
        long total = 0;
        await using (var output = new FileStream(tempPath, FileMode.CreateNew, FileAccess.Write, FileShare.None,
                         81920, FileOptions.Asynchronous | FileOptions.SequentialScan))
        {
            int read;
            while ((read = await input.ReadAsync(buffer, cancellationToken).ConfigureAwait(false)) != 0)
            {
                total = checked(total + read);
                if (total > maximumBytes) throw new InvalidDataException("다운로드 파일이 허용 크기를 초과했습니다.");
                hash.AppendData(buffer, 0, read);
                await output.WriteAsync(buffer.AsMemory(0, read), cancellationToken).ConfigureAwait(false);
                progress?.Report(new InstallProgress("다운로드", total, response.Content.Headers.ContentLength));
            }
            await output.FlushAsync(cancellationToken).ConfigureAwait(false);
        }
        var sha256 = Convert.ToHexString(hash.GetHashAndReset());
        File.Move(tempPath, destinationPath, true);
        return (total, sha256);
    }

    public static bool TryParseChecksum(string text, string assetName, out string? checksum)
    {
        checksum = null;
        foreach (var line in text.Split(new[] { '\r', '\n' }, StringSplitOptions.RemoveEmptyEntries))
        {
            var trimmed = line.Trim();
            var separator = trimmed.IndexOfAny(new[] { ' ', '\t', '*' });
            if (separator < 0) continue;
            var hash = trimmed[..separator];
            var name = trimmed[(separator + 1)..].TrimStart(' ', '\t', '*');
            if (name.Contains('/') || name.Contains('\\')) name = name[(Math.Max(name.LastIndexOf('/'), name.LastIndexOf('\\')) + 1)..];
            if (!name.Equals(assetName, StringComparison.OrdinalIgnoreCase)) continue;
            if (hash.Length != 64 || !hash.All(Uri.IsHexDigit)) return false;
            checksum = hash.ToUpperInvariant();
            return true;
        }
        return false;
    }
}
