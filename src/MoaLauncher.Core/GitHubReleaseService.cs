using System.Net;
using System.Net.Http.Headers;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace MoaLauncher.Core;

public sealed class GitHubReleaseService(HttpClient client, string cacheDirectory) : IReleaseService
{
    private static readonly TimeSpan RequestTimeout = TimeSpan.FromSeconds(20);
    private const int MaximumResponseBytes = 4 * 1024 * 1024;
    private readonly HttpClient _client = client ?? throw new ArgumentNullException(nameof(client));
    private readonly string _cacheDirectory = Path.GetFullPath(cacheDirectory ?? throw new ArgumentNullException(nameof(cacheDirectory)));

    public async Task<ReleaseResult> GetLatestAsync(AppDefinition app, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(app);
        if (!CatalogService.IsRepository(app.Repository)) throw new InvalidDataException("GitHub 저장소 주소를 먼저 설정해야 합니다.");
        var cachePath = CachePath(app.Repository);
        var cached = await ReadCacheAsync(cachePath, cancellationToken);
        using var request = new HttpRequestMessage(HttpMethod.Get, $"https://api.github.com/repos/{app.Repository}/releases/latest");
        request.Headers.Accept.Add(new MediaTypeWithQualityHeaderValue("application/vnd.github+json"));
        request.Headers.UserAgent.ParseAdd("MoaLauncher/0.1.0");
        request.Headers.TryAddWithoutValidation("X-GitHub-Api-Version", "2022-11-28");
        if (cached?.ETag is { Length: > 0 } etag && EntityTagHeaderValue.TryParse(etag, out var parsedTag))
            request.Headers.IfNoneMatch.Add(parsedTag);

        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeout.CancelAfter(RequestTimeout);
        try
        {
            using var response = await _client.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, timeout.Token);
            if (response.StatusCode == HttpStatusCode.NotModified)
            {
                if (cached is null) throw new InvalidDataException("GitHub가 캐시를 갱신하지 않았지만 저장된 릴리스 정보가 없습니다.");
                return new ReleaseResult(ParseRelease(cached.Body, app.Repository), IsCached: false);
            }
            if (response.StatusCode == HttpStatusCode.NotFound) throw new InvalidOperationException("GitHub 저장소에서 공개된 안정 버전을 찾을 수 없습니다.");
            if ((int)response.StatusCode is 403 or 429)
            {
                if (cached is not null) return new ReleaseResult(ParseRelease(cached.Body, app.Repository), IsCached: true);
                throw new InvalidOperationException("GitHub 요청 한도를 초과했습니다. 잠시 후 다시 확인해 주세요.");
            }
            if (!response.IsSuccessStatusCode)
            {
                if ((int)response.StatusCode >= 500 && cached is not null)
                    return new ReleaseResult(ParseRelease(cached.Body, app.Repository), IsCached: true);
                throw new InvalidOperationException($"GitHub 릴리스 조회에 실패했습니다 (HTTP {(int)response.StatusCode}).");
            }

            var body = await ReadBoundedBodyAsync(response.Content, timeout.Token);
            var release = ParseRelease(body, app.Repository);
            await WriteCacheAsync(cachePath, response.Headers.ETag?.ToString(), body, cancellationToken);
            return new ReleaseResult(release, IsCached: false);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested && cached is not null)
        {
            return new ReleaseResult(ParseRelease(cached.Body, app.Repository), IsCached: true);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            throw new TimeoutException("GitHub 릴리스 조회 시간이 초과되었습니다.");
        }
        catch (HttpRequestException) when (cached is not null)
        {
            return new ReleaseResult(ParseRelease(cached.Body, app.Repository), IsCached: true);
        }
    }

    private static AppRelease ParseRelease(string json, string repository)
    {
        try
        {
            using var document = JsonDocument.Parse(json);
            var root = document.RootElement;
            var tag = RequiredString(root, "tag_name");
            var name = OptionalString(root, "name") ?? tag;
            var notes = OptionalString(root, "body") ?? "";
            if (notes.Length > 100_000) notes = notes[..100_000];
            if (root.TryGetProperty("draft", out var draft) && draft.ValueKind == JsonValueKind.True ||
                root.TryGetProperty("prerelease", out var prerelease) && prerelease.ValueKind == JsonValueKind.True)
                throw new InvalidDataException("안정 버전 릴리스가 아닙니다.");
            if (!DateTimeOffset.TryParse(OptionalString(root, "published_at"), out var publishedAt))
                throw new InvalidDataException("릴리스 게시 시각이 올바르지 않습니다.");
            if (!Uri.TryCreate(RequiredString(root, "html_url"), UriKind.Absolute, out var pageUrl) ||
                !ValidateReleasePageUrl(repository, tag, pageUrl))
                throw new InvalidDataException("릴리스 페이지 주소가 올바르지 않습니다.");
            if (!root.TryGetProperty("assets", out var assetsElement) || assetsElement.ValueKind != JsonValueKind.Array || assetsElement.GetArrayLength() > 1000)
                throw new InvalidDataException("릴리스 파일 목록이 올바르지 않습니다.");
            var assets = new List<ReleaseAsset>();
            foreach (var asset in assetsElement.EnumerateArray())
            {
                var assetName = RequiredString(asset, "name");
                if (assetName.Length > 255 || assetName.IndexOfAny(['/', '\\']) >= 0) throw new InvalidDataException("릴리스 파일 이름이 올바르지 않습니다.");
                var downloadText = RequiredString(asset, "browser_download_url");
                if (!Uri.TryCreate(downloadText, UriKind.Absolute, out var downloadUrl) ||
                    !ReleaseRules.ValidateAssetUrl(repository, downloadUrl)) throw new InvalidDataException("릴리스 파일 주소가 현재 저장소와 일치하지 않습니다.");
                if (!asset.TryGetProperty("size", out var sizeElement) || !sizeElement.TryGetInt64(out var size) || size < 0)
                    throw new InvalidDataException("릴리스 파일 크기가 올바르지 않습니다.");
                var digest = OptionalString(asset, "digest");
                var sha256 = ParseDigest(digest);
                assets.Add(new ReleaseAsset(assetName, downloadUrl, size, sha256));
            }
            return new AppRelease(tag, name, notes, pageUrl, publishedAt, assets);
        }
        catch (JsonException ex)
        {
            throw new InvalidDataException("GitHub 릴리스 응답 형식이 올바르지 않습니다.", ex);
        }
    }

    private static string? ParseDigest(string? digest)
    {
        if (digest is null) return null;
        var match = Regex.Match(digest, "^sha256:([0-9a-fA-F]{64})$", RegexOptions.CultureInvariant);
        if (!match.Success) throw new InvalidDataException("릴리스 SHA-256 체크섬 형식이 올바르지 않습니다.");
        return match.Groups[1].Value.ToLowerInvariant();
    }

    private static bool ValidateReleasePageUrl(string repository, string tag, Uri url)
    {
        if (!url.IsAbsoluteUri || url.Scheme != Uri.UriSchemeHttps ||
            !string.Equals(url.Authority, "github.com", StringComparison.OrdinalIgnoreCase) ||
            !string.IsNullOrEmpty(url.Query) || !string.IsNullOrEmpty(url.Fragment)) return false;
        var expectedRepo = repository.Split('/');
        var parts = url.AbsolutePath.Split('/', StringSplitOptions.RemoveEmptyEntries).Select(Uri.UnescapeDataString).ToArray();
        return parts.Length == 5 && string.Equals(parts[0], expectedRepo[0], StringComparison.OrdinalIgnoreCase) &&
               string.Equals(parts[1], expectedRepo[1], StringComparison.OrdinalIgnoreCase) && parts[2] == "releases" &&
               parts[3] == "tag" && string.Equals(parts[4], tag, StringComparison.Ordinal);
    }

    private static async Task<string> ReadBoundedBodyAsync(HttpContent content, CancellationToken cancellationToken)
    {
        if (content.Headers.ContentLength is > MaximumResponseBytes)
            throw new InvalidDataException("GitHub 릴리스 응답이 허용된 크기보다 큽니다.");
        await using var input = await content.ReadAsStreamAsync(cancellationToken);
        using var output = new MemoryStream();
        var buffer = new byte[16 * 1024];
        while (true)
        {
            var count = await input.ReadAsync(buffer, cancellationToken);
            if (count == 0) break;
            if (output.Length + count > MaximumResponseBytes)
                throw new InvalidDataException("GitHub 릴리스 응답이 허용된 크기보다 큽니다.");
            output.Write(buffer, 0, count);
        }
        return System.Text.Encoding.UTF8.GetString(output.GetBuffer(), 0, checked((int)output.Length));
    }

    private static string RequiredString(JsonElement element, string name) =>
        element.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.String && !string.IsNullOrWhiteSpace(value.GetString())
            ? value.GetString()! : throw new InvalidDataException($"GitHub 릴리스 응답에 {name} 값이 없습니다.");

    private static string? OptionalString(JsonElement element, string name) =>
        element.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.String ? value.GetString() : null;

    private string CachePath(string repository)
    {
        var key = Convert.ToHexString(SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(repository.ToLowerInvariant()))).ToLowerInvariant();
        return Path.Combine(_cacheDirectory, key + ".json");
    }

    private static async Task<CachedResponse?> ReadCacheAsync(string path, CancellationToken cancellationToken)
    {
        if (!File.Exists(path)) return null;
        try
        {
            await using var stream = File.OpenRead(path);
            return await JsonSerializer.DeserializeAsync<CachedResponse>(stream, CatalogService.JsonOptions, cancellationToken);
        }
        catch (Exception ex) when (ex is IOException or JsonException or UnauthorizedAccessException)
        {
            return null;
        }
    }

    private async Task WriteCacheAsync(string path, string? etag, string body, CancellationToken cancellationToken)
    {
        Directory.CreateDirectory(_cacheDirectory);
        var tempPath = path + ".tmp-" + Guid.NewGuid().ToString("N");
        try
        {
            await using (var stream = new FileStream(tempPath, FileMode.CreateNew, FileAccess.Write, FileShare.None,
                             16 * 1024, FileOptions.Asynchronous | FileOptions.WriteThrough))
            {
                await JsonSerializer.SerializeAsync(stream, new CachedResponse(etag, body), CatalogService.JsonOptions, cancellationToken);
                await stream.FlushAsync(cancellationToken);
            }
            File.Move(tempPath, path, overwrite: true);
        }
        finally
        {
            if (File.Exists(tempPath)) File.Delete(tempPath);
        }
    }

    private sealed record CachedResponse(string? ETag, string Body);
}
