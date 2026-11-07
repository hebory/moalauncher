using System.Net;
using System.Runtime.InteropServices;
using MoaLauncher.Core;

namespace MoaLauncher.Core.Tests;

public sealed class ReleaseTests
{
    private const string Repository = "storytold/moaview";

    private static AppDefinition App() => new()
    {
        Id = "moaview", Name = "모아뷰", Repository = Repository,
        Packages = [new PackageDefinition { Architecture = "x64", Kind = "zip", AssetPattern = "MoaView-*-x64.zip", ExecutablePath = "MoaView.exe" }]
    };

    private static AppRelease Release(params ReleaseAsset[] assets) => new("v1.2.3", "모아뷰 1.2.3", "변경 사항",
        new Uri("https://github.com/storytold/moaview/releases/tag/v1.2.3"), DateTimeOffset.Parse("2026-10-31T12:00:00Z"), assets);

    private static ReleaseAsset Asset(string name = "MoaView-1.2.3-x64.zip") => new(name,
        new Uri($"https://github.com/{Repository}/releases/download/v1.2.3/{name}"), 1024, null);

    [Fact]
    public void SelectPackageChoosesRequestedArchitecture()
    {
        var app = App() with { Packages = [
            new PackageDefinition { Architecture = "x64", Kind = "zip", AssetPattern = "x64.zip", ExecutablePath = "MoaView.exe" },
            new PackageDefinition { Architecture = "arm64", Kind = "zip", AssetPattern = "arm64.zip", ExecutablePath = "MoaView.exe" }] };
        Assert.Equal("arm64", ReleaseRules.SelectPackage(app, Architecture.Arm64).Architecture);
        Assert.Throws<InvalidOperationException>(() => ReleaseRules.SelectPackage(app, Architecture.X86));
    }

    [Fact]
    public void SelectAssetRequiresExactlyOneStrictWildcardMatch()
    {
        var package = App().Packages[0];
        Assert.Equal("MoaView-1.2.3-x64.zip", ReleaseRules.SelectAsset(Release(Asset()), package).Name);
        Assert.Throws<InvalidDataException>(() => ReleaseRules.SelectAsset(Release(), package));
        Assert.Throws<InvalidDataException>(() => ReleaseRules.SelectAsset(Release(Asset(), Asset("MoaView-1.2.4-x64.zip")), package));
    }

    [Theory]
    [InlineData("v1.2.4", "1.2.3", true)]
    [InlineData("1.2.3", "1.2.3-rc.1", true)]
    [InlineData("1.2.3-rc.2", "1.2.3-rc.1", true)]
    [InlineData("1.2.3-alpha", "1.2.3-alpha.1", false)]
    [InlineData("1.2.3-beta", "1.2.3-alpha.9", true)]
    [InlineData("1.2.3+new", "1.2.3+old", false)]
    [InlineData("nightly", "1.2.3", false)]
    public void IsNewerUsesSemanticVersionPrecedence(string latest, string installed, bool expected) =>
        Assert.Equal(expected, ReleaseRules.IsNewer(latest, installed));

    [Theory]
    [InlineData("https://github.com/storytold/moaview/releases/download/v1/MoaView.zip", true)]
    [InlineData("http://github.com/storytold/moaview/releases/download/v1/MoaView.zip", false)]
    [InlineData("https://github.com/elsewhere/moaview/releases/download/v1/MoaView.zip", false)]
    [InlineData("https://user@github.com/storytold/moaview/releases/download/v1/MoaView.zip", false)]
    [InlineData("https://github.com:444/storytold/moaview/releases/download/v1/MoaView.zip", false)]
    [InlineData("https://github.com/storytold/moaview/releases/download/v1/MoaView.zip?token=x", false)]
    [InlineData("https://evil.example/storytold/moaview/releases/download/v1/MoaView.zip", false)]
    public void AssetUrlIsBoundToHttpsRepositoryReleasePath(string text, bool expected) =>
        Assert.Equal(expected, ReleaseRules.ValidateAssetUrl(Repository, new Uri(text)));

    [Fact]
    public async Task UsesETagAndReturnsNotModifiedCache()
    {
        var directory = NewCacheDirectory();
        try
        {
            var calls = 0;
            var handler = new StubHandler((request, _) =>
            {
                calls++;
                if (calls == 1) return Task.FromResult(JsonResponse(HttpStatusCode.OK, ValidJson(), "\"abc\""));
                Assert.Equal("\"abc\"", request.Headers.IfNoneMatch.Single().Tag);
                return Task.FromResult(new HttpResponseMessage(HttpStatusCode.NotModified));
            });
            var service = Service(directory, handler);
            Assert.False((await service.GetLatestAsync(App())).IsCached);
            var cached = await service.GetLatestAsync(App());
            Assert.False(cached.IsCached);
            Assert.Equal("sha256:" + new string('a', 64), "sha256:" + cached.Release.Assets[0].Sha256);
            Assert.Equal(2, calls);
        }
        finally { DeleteDirectory(directory); }
    }

    [Fact]
    public async Task FallsBackToCachedReleaseWhenOffline()
    {
        var directory = NewCacheDirectory();
        try
        {
            await Service(directory, new StubHandler((_, _) => Task.FromResult(JsonResponse(HttpStatusCode.OK, ValidJson())))).GetLatestAsync(App());
            var offline = Service(directory, new StubHandler((_, _) => throw new HttpRequestException("offline")));
            var result = await offline.GetLatestAsync(App());
            Assert.True(result.IsCached);
            Assert.Equal("v1.2.3", result.Release.Tag);
        }
        finally { DeleteDirectory(directory); }
    }

    [Theory]
    [InlineData(HttpStatusCode.Forbidden, "요청 한도")]
    [InlineData((HttpStatusCode)429, "요청 한도")]
    [InlineData(HttpStatusCode.NotFound, "안정 버전")]
    public async Task ReportsRateLimitAndMissingReleaseInKorean(HttpStatusCode status, string expectedText)
    {
        var service = Service(NewCacheDirectory(), new StubHandler((_, _) => Task.FromResult(new HttpResponseMessage(status))));
        var exception = await Assert.ThrowsAsync<InvalidOperationException>(() => service.GetLatestAsync(App()));
        Assert.Contains(expectedText, exception.Message);
    }

    [Fact]
    public async Task RejectsReleaseAssetOutsideRepositoryAndBadDigest()
    {
        var service = Service(NewCacheDirectory(), new StubHandler((_, _) => Task.FromResult(JsonResponse(HttpStatusCode.OK,
            ValidJson().Replace("github.com/storytold/moaview/releases", "github.com/other/moaview/releases", StringComparison.Ordinal)))));
        await Assert.ThrowsAsync<InvalidDataException>(() => service.GetLatestAsync(App()));
        var badDigest = ValidJson().Replace(new string('a', 64), "xyz", StringComparison.Ordinal);
        service = Service(NewCacheDirectory(), new StubHandler((_, _) => Task.FromResult(JsonResponse(HttpStatusCode.OK, badDigest))));
        await Assert.ThrowsAsync<InvalidDataException>(() => service.GetLatestAsync(App()));
    }

    [Fact]
    public async Task RejectsOversizedReleaseResponse()
    {
        var service = Service(NewCacheDirectory(), new StubHandler((_, _) => Task.FromResult(JsonResponse(HttpStatusCode.OK,
            new string(' ', 4 * 1024 * 1024 + 1)))));
        var exception = await Assert.ThrowsAsync<InvalidDataException>(() => service.GetLatestAsync(App()));
        Assert.Contains("허용된 크기", exception.Message);
    }

    [Fact]
    public async Task UsesStaleCacheOnRateLimitButNeverOnNotFound()
    {
        var directory = NewCacheDirectory();
        try
        {
            await Service(directory, new StubHandler((_, _) => Task.FromResult(JsonResponse(HttpStatusCode.OK, ValidJson())))).GetLatestAsync(App());
            var limited = await Service(directory, new StubHandler((_, _) => Task.FromResult(new HttpResponseMessage((HttpStatusCode)429))))
                .GetLatestAsync(App());
            Assert.True(limited.IsCached);
            var missing = Service(directory, new StubHandler((_, _) => Task.FromResult(new HttpResponseMessage(HttpStatusCode.NotFound))));
            await Assert.ThrowsAsync<InvalidOperationException>(() => missing.GetLatestAsync(App()));
        }
        finally { DeleteDirectory(directory); }
    }

    private static GitHubReleaseService Service(string directory, HttpMessageHandler handler) =>
        new(new HttpClient(handler), directory);

    private static HttpResponseMessage JsonResponse(HttpStatusCode status, string json, string? etag = null)
    {
        var response = new HttpResponseMessage(status) { Content = new StringContent(json) };
        if (etag is not null) response.Headers.ETag = System.Net.Http.Headers.EntityTagHeaderValue.Parse(etag);
        return response;
    }

    private static string ValidJson() => $$"""
        {
          "tag_name": "v1.2.3",
          "name": "MoaView 1.2.3",
          "body": "Release notes",
          "html_url": "https://github.com/{{Repository}}/releases/tag/v1.2.3",
          "published_at": "2026-10-31T12:00:00Z",
          "draft": false,
          "prerelease": false,
          "assets": [{
            "name": "MoaView-1.2.3-x64.zip",
            "browser_download_url": "https://github.com/{{Repository}}/releases/download/v1.2.3/MoaView-1.2.3-x64.zip",
            "size": 1024,
            "digest": "sha256:{{new string('a', 64)}}"
          }]
        }
        """;

    private static string NewCacheDirectory() => Path.Combine(Path.GetTempPath(), "moa-release-test-" + Guid.NewGuid().ToString("N"));
    private static void DeleteDirectory(string path) { if (Directory.Exists(path)) Directory.Delete(path, recursive: true); }

    private sealed class StubHandler(Func<HttpRequestMessage, CancellationToken, Task<HttpResponseMessage>> send) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken) => send(request, cancellationToken);
    }
}
