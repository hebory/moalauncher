using System.IO.Compression;
using System.Net;
using System.Net.Http.Headers;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using MoaLauncher.Core;

namespace MoaLauncher.Core.Tests;

public sealed class PrivateReleaseTests
{
    private const string Repository = "owner/private-app";
    private const string Token = "secret-test-github-token";
    private const string ApiUrlPrefix = "https://api.github.com/repos/" + Repository + "/releases/assets/";

    [Fact]
    public async Task PrivateReleaseUsesTokenAndNeitherReadsNorWritesDiskCache()
    {
        using var temp = new TemporaryDirectory();
        var cacheDirectory = Path.Combine(temp.Path, "cache");
        Directory.CreateDirectory(cacheDirectory);
        var cachePath = CachePath(cacheDirectory);
        var oldCache = JsonSerializer.Serialize(new { ETag = "\"old\"", Body = ReleaseJson("v1.0.0", isPrivate: false) }, CatalogService.JsonOptions);
        await File.WriteAllTextAsync(cachePath, oldCache);
        var requests = new List<HttpRequestMessage>();
        using var client = new HttpClient(new StubHandler((request, _) =>
        {
            requests.Add(request);
            return Task.FromResult(Json(HttpStatusCode.OK, ReleaseJson("v2.0.0", isPrivate: true), request));
        }));
        var service = new GitHubReleaseService(client, cacheDirectory, new StaticTokenProvider(Token));

        var result = await service.GetLatestAsync(PrivateApp());

        Assert.Equal("v2.0.0", result.Release.Tag);
        Assert.True(result.Release.IsPrivate);
        Assert.False(result.IsCached);
        Assert.Equal(Token, requests[0].Headers.Authorization?.Parameter);
        Assert.Empty(requests[0].Headers.IfNoneMatch);
        Assert.Equal(new Uri(ApiUrlPrefix + "42"), result.Release.Assets[0].ApiUrl);
        Assert.Equal(oldCache, await File.ReadAllTextAsync(cachePath));
    }

    [Fact]
    public void PrivateAssetApiUrlRequiresRealReposRouteAndSameRepository()
    {
        Assert.True(ReleaseRules.ValidatePrivateAssetApiUrl(Repository,
            new Uri(ApiUrlPrefix + "42")));
        Assert.False(ReleaseRules.ValidatePrivateAssetApiUrl(Repository,
            new Uri("https://api.github.com/owner/private-app/releases/assets/42")));
        Assert.False(ReleaseRules.ValidatePrivateAssetApiUrl(Repository,
            new Uri("https://api.github.com/repos/other/private-app/releases/assets/42")));
    }

    [Fact]
    public async Task PublicReleaseStaysAnonymousWhenTokenProviderExists()
    {
        using var temp = new TemporaryDirectory();
        HttpRequestMessage? sentRequest = null;
        using var client = new HttpClient(new StubHandler((request, _) =>
        {
            sentRequest = request;
            return Task.FromResult(Json(HttpStatusCode.OK, ReleaseJson("v2.0.0", isPrivate: false), request));
        }));

        var release = await new GitHubReleaseService(client, Path.Combine(temp.Path, "cache"), new StaticTokenProvider(Token))
            .GetLatestAsync(PrivateApp() with { IsPrivate = false });

        Assert.NotNull(sentRequest);
        Assert.Null(sentRequest!.Headers.Authorization);
        Assert.False(release.Release.IsPrivate);
        Assert.Null(release.Release.Assets[0].ApiUrl);
    }

    [Theory]
    [InlineData(HttpStatusCode.Unauthorized)]
    [InlineData(HttpStatusCode.Forbidden)]
    [InlineData(HttpStatusCode.NotFound)]
    public async Task PrivateAccessErrorsNeverExposeTokenOrResponseBody(HttpStatusCode status)
    {
        using var temp = new TemporaryDirectory();
        var responseBody = $"private diagnostic {Token} signed-download-secret";
        using var client = new HttpClient(new StubHandler((request, _) =>
            Task.FromResult(Json(status, responseBody, request))));
        var service = new GitHubReleaseService(client, Path.Combine(temp.Path, "cache"), new StaticTokenProvider(Token));

        var exception = await Assert.ThrowsAsync<InvalidOperationException>(() => service.GetLatestAsync(PrivateApp()));

        Assert.DoesNotContain(Token, exception.ToString(), StringComparison.Ordinal);
        Assert.DoesNotContain(responseBody, exception.ToString(), StringComparison.Ordinal);
        Assert.False(Directory.Exists(Path.Combine(temp.Path, "cache")));
    }

    [Fact]
    public async Task MissingOrMalformedTokenFailsBeforeNetworkAndCacheAccess()
    {
        using var temp = new TemporaryDirectory();
        var requests = new List<HttpRequestMessage>();
        using var client = new HttpClient(new StubHandler((request, _) =>
        {
            requests.Add(request);
            return Task.FromResult(Json(HttpStatusCode.OK, ReleaseJson("v2.0.0", isPrivate: true), request));
        }));

        foreach (var token in new string?[] { null, " token with spaces ", "token\rwith-control", "secret\0token" })
        {
            var service = new GitHubReleaseService(client, Path.Combine(temp.Path, "cache"), new StaticTokenProvider(token));
            var error = await Assert.ThrowsAsync<InvalidOperationException>(() => service.GetLatestAsync(PrivateApp()));
            Assert.DoesNotContain("token with spaces", error.ToString(), StringComparison.Ordinal);
        }

        Assert.Empty(requests);
        Assert.False(Directory.Exists(Path.Combine(temp.Path, "cache")));
    }

    [Fact]
    public async Task PrivateLatestRejectsRewrittenRequestUri()
    {
        using var temp = new TemporaryDirectory();
        using var client = new HttpClient(new StubHandler((request, _) =>
        {
            request.RequestUri = new Uri("https://api.github.com/repos/another/private-app/releases/latest");
            return Task.FromResult(Json(HttpStatusCode.OK, ReleaseJson("v2.0.0", isPrivate: true), request));
        }));
        var service = new GitHubReleaseService(client, Path.Combine(temp.Path, "cache"), new StaticTokenProvider(Token));

        await Assert.ThrowsAsync<InvalidOperationException>(() => service.GetLatestAsync(PrivateApp()));
        Assert.False(Directory.Exists(Path.Combine(temp.Path, "cache")));
    }

    [Fact]
    public async Task PrivateTransportFailuresDoNotExposeCredentialOrSignedUrl()
    {
        using var temp = new TemporaryDirectory();
        const string signedUrl = "https://release-assets.githubusercontent.com/app.zip?signature=private-secret";
        using var client = new HttpClient(new StubHandler((_, _) =>
            throw new HttpRequestException($"{Token} {signedUrl}")));
        var provider = new StaticTokenProvider(Token);
        var releaseError = await Assert.ThrowsAsync<InvalidOperationException>(() =>
            new GitHubReleaseService(client, Path.Combine(temp.Path, "cache"), provider).GetLatestAsync(PrivateApp()));
        var downloadError = await Assert.ThrowsAsync<InvalidOperationException>(() =>
            new VerifiedDownloader(client, provider).DownloadAssetAsync(PrivateAsset("app.zip", [1], 7),
                Repository, true, Path.Combine(temp.Path, "download.zip")));

        foreach (var error in new[] { releaseError, downloadError })
        {
            Assert.DoesNotContain(Token, error.ToString(), StringComparison.Ordinal);
            Assert.DoesNotContain(signedUrl, error.ToString(), StringComparison.Ordinal);
        }
        Assert.False(File.Exists(Path.Combine(temp.Path, "download.zip")));
    }

    [Fact]
    public async Task PrivateReleaseRejectsAssetApiUrlForAnotherRepository()
    {
        using var temp = new TemporaryDirectory();
        var json = ReleaseJson("v2.0.0", isPrivate: true)
            .Replace(ApiUrlPrefix, "https://api.github.com/repos/other/private-app/releases/assets/", StringComparison.Ordinal);
        using var client = new HttpClient(new StubHandler((request, _) => Task.FromResult(Json(HttpStatusCode.OK, json, request))));
        var service = new GitHubReleaseService(client, Path.Combine(temp.Path, "cache"), new StaticTokenProvider(Token));

        var error = await Assert.ThrowsAsync<InvalidDataException>(() => service.GetLatestAsync(PrivateApp()));

        Assert.Contains("API 주소", error.Message, StringComparison.Ordinal);
    }

    [Fact]
    public async Task PrivateZipInstallAndUpdateUseAuthenticatedApiThenUnsignedRedirect()
    {
        using var temp = new TemporaryDirectory();
        var v1 = CreateZip("version 1");
        var v2 = CreateZip("version 2");
        var requests = new List<RequestSnapshot>();
        using var client = new HttpClient(new StubHandler((request, _) =>
        {
            requests.Add(RequestSnapshot.From(request));
            var id = request.RequestUri!.AbsolutePath.Split('/').Last();
            if (id == "2")
            {
                var redirect = new HttpResponseMessage(HttpStatusCode.Redirect);
                redirect.Headers.Location = new Uri("https://release-assets.githubusercontent.com/private/app.zip?signature=temporary");
                redirect.RequestMessage = request;
                return Task.FromResult(redirect);
            }
            var content = request.RequestUri!.Host.Equals("release-assets.githubusercontent.com", StringComparison.OrdinalIgnoreCase)
                ? v2 : id == "1" ? v1 : Array.Empty<byte>();
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK)
            {
                RequestMessage = request,
                Content = new ByteArrayContent(content)
            });
        }));
        var service = new InstallationService(client, temp.Path, Path.Combine(temp.Path, "local"), new StaticTokenProvider(Token));

        var first = await service.InstallAsync(PrivateApp(), PrivateRelease("v1.0.0", v1, assetId: 1));
        var second = await service.InstallAsync(PrivateApp(), PrivateRelease("v2.0.0", v2, assetId: 2));

        Assert.Equal("v1.0.0", first.Version);
        Assert.Equal("v2.0.0", second.Version);
        Assert.Equal("version 2", await File.ReadAllTextAsync(second.ExecutablePath));
        Assert.Equal(new[] { "https://api.github.com", "https://api.github.com", "https://release-assets.githubusercontent.com" },
            requests.Select(request => request.Uri.GetLeftPart(UriPartial.Authority)));
        Assert.All(requests.Take(2), request => Assert.Equal("Bearer " + Token, request.Authorization));
        Assert.Null(requests[2].Authorization);
        Assert.Null(requests[2].Cookie);
        Assert.Equal("application/octet-stream", requests[1].Accept);
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task PrivateZipInstallDownloadsChecksumSidecarThroughAssetApi(bool useSumsFile)
    {
        using var temp = new TemporaryDirectory();
        var archive = CreateZip("checksummed");
        var sha = Convert.ToHexString(SHA256.HashData(archive)).ToLowerInvariant();
        var checksumName = useSumsFile ? "SHA256SUMS.txt" : "app.zip.sha256";
        var checksum = useSumsFile ? $"{sha}  app.zip\n" : $"{sha}\n";
        var requests = new List<RequestSnapshot>();
        using var client = new HttpClient(new StubHandler((request, _) =>
        {
            requests.Add(RequestSnapshot.From(request));
            var content = request.RequestUri!.AbsolutePath.EndsWith("/11", StringComparison.Ordinal)
                ? archive : Encoding.UTF8.GetBytes(checksum);
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK)
            {
                RequestMessage = request,
                Content = new ByteArrayContent(content)
            });
        }));
        var assets = new[]
        {
            PrivateAsset("app.zip", archive, 11, includeHash: false),
            PrivateTextAsset(checksumName, checksum, 12)
        };
        var service = new InstallationService(client, temp.Path, Path.Combine(temp.Path, "local"), new StaticTokenProvider(Token));

        var installed = await service.InstallAsync(PrivateApp(), PrivateRelease("v1.0.0", assets));

        Assert.Equal("checksummed", await File.ReadAllTextAsync(installed.ExecutablePath));
        Assert.Equal(2, requests.Count);
        Assert.All(requests, request =>
        {
            Assert.StartsWith("https://api.github.com/repos/" + Repository + "/releases/assets/", request.Uri.AbsoluteUri, StringComparison.Ordinal);
            Assert.Equal("Bearer " + Token, request.Authorization);
        });
    }

    [Fact]
    public async Task InstallationRejectsMismatchedVisibilityAndCrossRepositoryAssetBeforeNetwork()
    {
        using var temp = new TemporaryDirectory();
        var requests = new List<HttpRequestMessage>();
        using var client = new HttpClient(new StubHandler((request, _) =>
        {
            requests.Add(request);
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK) { RequestMessage = request, Content = new ByteArrayContent([1]) });
        }));
        var service = new InstallationService(client, temp.Path, Path.Combine(temp.Path, "local"), new StaticTokenProvider(Token));
        var package = CreateZip("must not install");

        await Assert.ThrowsAsync<InvalidOperationException>(() => service.InstallAsync(
            PrivateApp() with { IsPrivate = false }, PrivateRelease("v1.0.0", package, assetId: 1)));
        await Assert.ThrowsAsync<InvalidDataException>(() => service.InstallAsync(PrivateApp(), PrivateRelease("v1.0.0", package,
            assetId: 1, apiUrl: new Uri("https://api.github.com/repos/another/private-app/releases/assets/1"))));

        Assert.Empty(requests);
        Assert.False(Directory.Exists(Path.Combine(temp.Path, "apps")));
    }

    [Fact]
    public async Task PrivateRedirectRejectsUnknownHostWithoutSendingTokenThere()
    {
        using var temp = new TemporaryDirectory();
        var requests = new List<RequestSnapshot>();
        using var client = new HttpClient(new StubHandler((request, _) =>
        {
            requests.Add(RequestSnapshot.From(request));
            var response = new HttpResponseMessage(HttpStatusCode.Redirect);
            response.Headers.Location = new Uri("https://attacker.example/private?signature=secret");
            response.RequestMessage = request;
            return Task.FromResult(response);
        }));
        var downloader = new VerifiedDownloader(client, new StaticTokenProvider(Token));

        await Assert.ThrowsAsync<InvalidDataException>(() => downloader.DownloadAssetAsync(
            PrivateAsset("app.zip", [1], 7), Repository, true, Path.Combine(temp.Path, "download.zip")));

        Assert.Single(requests);
        Assert.Equal("Bearer " + Token, requests[0].Authorization);
        Assert.False(File.Exists(Path.Combine(temp.Path, "download.zip")));
    }

    [Theory]
    [InlineData(HttpStatusCode.Unauthorized)]
    [InlineData(HttpStatusCode.Forbidden)]
    [InlineData(HttpStatusCode.NotFound)]
    public async Task PrivateAssetErrorsNeverExposeTokenOrResponseBody(HttpStatusCode status)
    {
        using var temp = new TemporaryDirectory();
        var responseBody = $"private diagnostic {Token} signed-download-secret";
        using var client = new HttpClient(new StubHandler((request, _) => Task.FromResult(Json(status, responseBody, request))));
        var downloader = new VerifiedDownloader(client, new StaticTokenProvider(Token));

        var error = await Assert.ThrowsAsync<InvalidOperationException>(() => downloader.DownloadAssetAsync(
            PrivateAsset("app.zip", [1], 7), Repository, true, Path.Combine(temp.Path, "download.zip")));

        Assert.DoesNotContain(Token, error.ToString(), StringComparison.Ordinal);
        Assert.DoesNotContain(responseBody, error.ToString(), StringComparison.Ordinal);
        Assert.False(File.Exists(Path.Combine(temp.Path, "download.zip")));
    }

    private static AppDefinition PrivateApp() => new()
    {
        Id = "private-app", Name = "Private App", Repository = Repository, IsPrivate = true,
        Packages = [new PackageDefinition { Architecture = "x64", Kind = "zip", AssetPattern = "app.zip", ExecutablePath = "PrivateApp.exe" }]
    };

    private static AppRelease PrivateRelease(string version, byte[] package, int assetId) =>
        PrivateRelease(version, [PrivateAsset("app.zip", package, assetId)]);

    private static AppRelease PrivateRelease(string version, IReadOnlyList<ReleaseAsset> assets) => new(
        version, version, "", new Uri($"https://github.com/{Repository}/releases/tag/{version}"),
        DateTimeOffset.UtcNow, assets, IsPrivate: true);

    private static AppRelease PrivateRelease(string version, byte[] package, int assetId, Uri apiUrl) =>
        PrivateRelease(version, [PrivateAsset("app.zip", package, assetId) with { ApiUrl = apiUrl }]);

    private static ReleaseAsset PrivateAsset(string name, byte[] bytes, int assetId, bool includeHash = true) => new(
        name, BrowserUrl(name, "v1.0.0"), bytes.Length,
        includeHash ? Convert.ToHexString(SHA256.HashData(bytes)) : null,
        new Uri(ApiUrlPrefix + assetId));

    private static ReleaseAsset PrivateTextAsset(string name, string text, int assetId) => new(
        name, BrowserUrl(name, "v1.0.0"), Encoding.UTF8.GetByteCount(text), null,
        new Uri(ApiUrlPrefix + assetId));

    private static Uri BrowserUrl(string name, string version) =>
        new($"https://github.com/{Repository}/releases/download/{version}/{name}");

    private static byte[] CreateZip(string contents)
    {
        using var stream = new MemoryStream();
        using (var archive = new ZipArchive(stream, ZipArchiveMode.Create, leaveOpen: true))
        {
            var file = archive.CreateEntry("PrivateApp.exe", CompressionLevel.NoCompression);
            using var output = file.Open();
            output.Write(Encoding.UTF8.GetBytes(contents));
        }
        return stream.ToArray();
    }

    private static string ReleaseJson(string tag, bool isPrivate)
    {
        var version = tag.TrimStart('v', 'V');
        return $$"""
        {
          "tag_name": "{{tag}}",
          "name": "Private release",
          "body": "notes",
          "html_url": "https://github.com/{{Repository}}/releases/tag/{{tag}}",
          "published_at": "2026-11-12T00:00:00Z",
          "draft": false,
          "prerelease": false,
          "assets": [{
            "url": "{{ApiUrlPrefix}}42",
            "name": "app.zip",
            "browser_download_url": "https://github.com/{{Repository}}/releases/download/{{version}}/app.zip",
            "size": 1024,
            "digest": "sha256:{{new string('a', 64)}}"
          }]
        }
        """;
    }

    private static HttpResponseMessage Json(HttpStatusCode status, string json, HttpRequestMessage request) => new(status)
    {
        RequestMessage = request,
        Content = new StringContent(json)
    };

    private static string CachePath(string directory) => Path.Combine(directory,
        Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(Repository.ToLowerInvariant()))).ToLowerInvariant() + ".json");

    private sealed record RequestSnapshot(Uri Uri, string? Authorization, string? Cookie, string? Accept)
    {
        public static RequestSnapshot From(HttpRequestMessage request) => new(request.RequestUri!,
            request.Headers.Authorization?.ToString(),
            request.Headers.Contains("Cookie") ? string.Join(";", request.Headers.GetValues("Cookie")) : null,
            string.Join(";", request.Headers.Accept.Select(value => value.MediaType)));
    }

    private sealed class StaticTokenProvider(string? token) : IGitHubTokenProvider
    {
        public string? GetToken() => token;
    }

    private sealed class StubHandler(Func<HttpRequestMessage, CancellationToken, Task<HttpResponseMessage>> handler) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken) =>
            handler(request, cancellationToken);
    }

    private sealed class TemporaryDirectory : IDisposable
    {
        public string Path { get; } = System.IO.Path.Combine(System.IO.Path.GetTempPath(), "moa-private-release-" + Guid.NewGuid().ToString("N"));
        public TemporaryDirectory() => Directory.CreateDirectory(Path);
        public void Dispose() { if (Directory.Exists(Path)) Directory.Delete(Path, recursive: true); }
    }
}
