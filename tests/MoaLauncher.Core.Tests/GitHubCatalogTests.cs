using System.Net;
using System.Net.Http.Headers;
using MoaLauncher.Core;

namespace MoaLauncher.Core.Tests;

public sealed class GitHubCatalogTests
{
    private const string Repository = "owner/project";

    [Fact]
    public async Task ResolveVisibilityMarksRepositoryPrivateUsingProvidedToken()
    {
        var provider = new TestTokenProvider("test-token");
        var handler = new RecordingHandler((request, _) => Task.FromResult(VisibilityResponse(request, privateRepository: true)));
        var service = new GitHubCatalogService(new HttpClient(handler), provider);

        var result = await service.ResolveVisibilityAsync(CreateApp());

        Assert.True(result.IsPrivate);
        var request = Assert.Single(handler.Requests);
        Assert.Equal(HttpMethod.Get, request.Method);
        Assert.Equal($"https://api.github.com/repos/{Repository}", request.RequestUri!.AbsoluteUri);
        Assert.Equal(new AuthenticationHeaderValue("Bearer", "test-token"), request.Headers.Authorization);
        Assert.Equal(1, provider.ReadCount);
    }

    [Fact]
    public async Task ResolveVisibilityLeavesAlreadyPrivateOrUnpublishedAppsAlone()
    {
        var provider = new TestTokenProvider("test-token");
        var handler = new RecordingHandler((_, _) => throw new InvalidOperationException("No request expected."));
        var service = new GitHubCatalogService(new HttpClient(handler), provider);
        var privateApp = CreateApp() with { IsPrivate = true };
        var placeholder = CreateApp() with { Repository = "" };

        Assert.Same(privateApp, await service.ResolveVisibilityAsync(privateApp));
        Assert.Same(placeholder, await service.ResolveVisibilityAsync(placeholder));
        Assert.Empty(handler.Requests);
        Assert.Equal(0, provider.ReadCount);
    }

    [Fact]
    public async Task PublicExportExcludesMarkedAndUndiscoverablePrivateRepositoriesWithoutAuthentication()
    {
        var provider = new TestTokenProvider("must-not-be-used");
        var handler = new RecordingHandler((request, _) =>
        {
            Assert.Null(request.Headers.Authorization);
            var repository = request.RequestUri!.AbsolutePath;
            return Task.FromResult(repository.EndsWith("/owner/public", StringComparison.Ordinal)
                ? VisibilityResponse(request, privateRepository: false)
                : new HttpResponseMessage(HttpStatusCode.NotFound) { RequestMessage = request });
        });
        var service = new GitHubCatalogService(new HttpClient(handler), provider);
        var markedPrivate = CreateApp("marked-private") with { IsPrivate = true };
        var publicApp = CreateApp("public") with { Repository = "owner/public" };
        var mislabelledPrivate = CreateApp("mislabelled-private") with { Repository = "owner/private" };
        var placeholder = CreateApp("placeholder") with { Repository = "" };

        var exported = await service.CreatePublicExportAsync([markedPrivate, publicApp, mislabelledPrivate, placeholder]);

        var actual = Assert.Single(exported.Apps);
        Assert.Equal(publicApp.Id, actual.Id);
        Assert.Equal(2, handler.Requests.Count);
        Assert.All(handler.Requests, request => Assert.Null(request.Headers.Authorization));
        Assert.Equal(0, provider.ReadCount);
    }

    [Fact]
    public async Task PublicExportFailsClosedWhenRepositoryVisibilityCannotBeChecked()
    {
        const string responseBody = "sensitive upstream details";
        var handler = new RecordingHandler((_, _) => throw new HttpRequestException(responseBody));
        var service = new GitHubCatalogService(new HttpClient(handler), new TestTokenProvider("never-used"));

        var exception = await Assert.ThrowsAsync<InvalidOperationException>(() =>
            service.CreatePublicExportAsync([CreateApp()]));

        Assert.Contains("GitHub 저장소 정보를 조회하지 못했습니다", exception.Message);
        Assert.DoesNotContain(responseBody, exception.ToString());
    }

    [Fact]
    public async Task ResolveVisibilitySanitizesMalformedTokenAndCredentialProviderFailures()
    {
        var requests = new RecordingHandler((request, _) =>
            Task.FromResult(VisibilityResponse(request, privateRepository: true)));
        var serviceWithMalformedToken = new GitHubCatalogService(new HttpClient(requests), new TestTokenProvider("bad token-secret"));
        var malformedTokenError = await Assert.ThrowsAsync<InvalidOperationException>(() =>
            serviceWithMalformedToken.ResolveVisibilityAsync(CreateApp()));
        Assert.DoesNotContain("token-secret", malformedTokenError.ToString(), StringComparison.Ordinal);
        Assert.Empty(requests.Requests);

        var providerSecret = "credential-provider-secret";
        var serviceWithBrokenProvider = new GitHubCatalogService(new HttpClient(requests), new ThrowingTokenProvider(providerSecret));
        var providerError = await Assert.ThrowsAsync<InvalidOperationException>(() =>
            serviceWithBrokenProvider.ResolveVisibilityAsync(CreateApp()));
        Assert.DoesNotContain(providerSecret, providerError.ToString(), StringComparison.Ordinal);
        Assert.Empty(requests.Requests);
    }

    [Theory]
    [InlineData("{}")]
    [InlineData("{\"private\":null}")]
    [InlineData("{\"private\":\"false\"}")]
    public async Task VisibilityRequiresBooleanPrivateProperty(string json)
    {
        var handler = new RecordingHandler((request, _) => Task.FromResult(JsonResponse(HttpStatusCode.OK, json, request)));
        var service = new GitHubCatalogService(new HttpClient(handler));

        await Assert.ThrowsAsync<InvalidDataException>(() => service.ResolveVisibilityAsync(CreateApp()));
    }

    private static AppDefinition CreateApp(string id = "private-app") => new()
    {
        Id = id,
        Name = "테스트 프로그램",
        Category = "개인 도구",
        AccentColor = "#436F5D",
        Monogram = "T",
        Repository = Repository,
        Packages = [new PackageDefinition
        {
            Architecture = "x64", Kind = "zip", AssetPattern = "app-*.zip", ExecutablePath = "app.exe"
        }]
    };

    private static HttpResponseMessage VisibilityResponse(HttpRequestMessage request, bool privateRepository) =>
        JsonResponse(HttpStatusCode.OK, $"{{\"private\":{privateRepository.ToString().ToLowerInvariant()}}}", request);

    private static HttpResponseMessage JsonResponse(HttpStatusCode status, string content, HttpRequestMessage? request = null) => new(status)
    {
        Content = new StringContent(content),
        RequestMessage = request
    };

    private sealed class TestTokenProvider(string? token) : IGitHubTokenProvider
    {
        public int ReadCount { get; private set; }
        public string? GetToken() { ReadCount++; return token; }
    }

    private sealed class ThrowingTokenProvider(string secret) : IGitHubTokenProvider
    {
        public string? GetToken() => throw new InvalidOperationException(secret);
    }

    private sealed class RecordingHandler(Func<HttpRequestMessage, CancellationToken, Task<HttpResponseMessage>> respond)
        : HttpMessageHandler
    {
        public List<HttpRequestMessage> Requests { get; } = [];

        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            Requests.Add(request);
            return respond(request, cancellationToken);
        }
    }
}
