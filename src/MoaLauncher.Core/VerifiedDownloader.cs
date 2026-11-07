using System.Security.Cryptography;

namespace MoaLauncher.Core;

/// <summary>Downloads bounded HTTPS GitHub release files and verifies their SHA-256.</summary>
public sealed class VerifiedDownloader(HttpClient client)
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
                if (response.Content.Headers.ContentLength is long length && length > maximumBytes)
                    throw new InvalidDataException("다운로드 파일이 허용 크기를 초과했습니다.");

                await using var input = await response.Content.ReadAsStreamAsync(timeout.Token).ConfigureAwait(false);
                using var hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
                var buffer = new byte[81920];
                long total = 0;
                {
                    await using var output = new FileStream(tempPath, FileMode.CreateNew, FileAccess.Write, FileShare.None, 81920, FileOptions.Asynchronous | FileOptions.SequentialScan);
                    int read;
                    while ((read = await input.ReadAsync(buffer, timeout.Token).ConfigureAwait(false)) != 0)
                    {
                        total = checked(total + read);
                        if (total > maximumBytes) throw new InvalidDataException("다운로드 파일이 허용 크기를 초과했습니다.");
                        hash.AppendData(buffer, 0, read);
                        await output.WriteAsync(buffer.AsMemory(0, read), timeout.Token).ConfigureAwait(false);
                        progress?.Report(new InstallProgress("다운로드", total, response.Content.Headers.ContentLength));
                    }
                    await output.FlushAsync(timeout.Token).ConfigureAwait(false);
                }
                var sha256 = Convert.ToHexString(hash.GetHashAndReset());
                File.Move(tempPath, destinationPath, true);
                return (total, sha256);
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
