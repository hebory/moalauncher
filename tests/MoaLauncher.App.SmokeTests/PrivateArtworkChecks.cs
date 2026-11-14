using System.IO;
using System.Net;
using System.Net.Http;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using MoaLauncher.App;
using MoaLauncher.Core;

namespace MoaLauncher.App.SmokeTests;

internal static class PrivateArtworkChecks
{
    private const string Repository = "fixture-owner/fixture-repository";
    private const string Token = "dummy-private-artwork-token";
    private const int MaximumBytes = 8 * 1024 * 1024;
    private static string IconUrl => $"https://raw.githubusercontent.com/{Repository}/main/icon.png";
    private static string CoverUrl => $"https://raw.githubusercontent.com/{Repository}/main/cover.png";

    internal static async Task RunAsync(string dataDirectory)
    {
        var root = Path.GetFullPath(Path.Combine(dataDirectory, "private-artwork-smoke"));
        var cache = Path.Combine(root, "cache");
        Directory.CreateDirectory(cache);
        var png = CreateTinyPng();
        var privateApp = App(isPrivate: true);
        var credentials = new MutableToken(Token);
        var requests = new List<(Uri Uri, string? Authorization, string? Accept)>();
        using (var http = new HttpClient(new StubHandler((request, _) =>
        {
            requests.Add(Snapshot(request));
            return Task.FromResult(PngResponse(request, png));
        })))
        {
            var artwork = new AppArtworkService(http, cache, credentials);
            foreach (var (file, url) in new[] { ("icon.png", IconUrl), ("cover.png", CoverUrl) })
            {
                var image = await artwork.LoadAsync(privateApp, url);
                Require(image is { IsFrozen: true, PixelWidth: 2, PixelHeight: 2 }, $"Private {file} was not decoded.");
            }
        }
        Require(requests.Count == 2, "Private icon and cover did not make exactly two requests.");
        foreach (var request in requests)
        {
            Require(request.Uri.Host == "api.github.com" && request.Uri.AbsolutePath == $"/repos/{Repository}/contents/{Path.GetFileName(request.Uri.AbsolutePath)}",
                "Private artwork request escaped the repository contents API.");
            Require(request.Uri.Query == "?ref=main", "The image branch was not passed to the Contents API.");
            Require(request.Authorization == "Bearer " + Token, "Private API request did not use the current token.");
            Require(request.Accept?.Contains("application/vnd.github.raw+json", StringComparison.Ordinal) == true,
                "Private API request did not request raw image content.");
        }
        Require(!Directory.EnumerateFiles(cache, "*", SearchOption.AllDirectories).Any(), "Private artwork was written to disk cache.");

        var unsafeCalls = 0;
        string? publicAuthorization = null;
        using (var http = new HttpClient(new StubHandler((request, _) =>
        {
            unsafeCalls++;
            publicAuthorization = request.Headers.Authorization?.ToString();
            return Task.FromResult(PngResponse(request, png));
        })))
        {
            var artwork = new AppArtworkService(http, cache, credentials);
            foreach (var bad in new[]
            {
                "https://raw.githubusercontent.com/other-owner/fixture-repository/main/icon.png",
                "https://raw.githubusercontent.com/fixture-owner/other-repository/main/icon.png",
                "https://raw.githubusercontent.com/fixture-owner/fixture-repository/main/../secret.png",
                "https://example.com/fixture-owner/fixture-repository/main/icon.png",
                "https://raw.githubusercontent.com/fixture-owner/fixture-repository/main/icon.png?download=1",
                "https://api.github.com/repos/other-owner/fixture-repository/contents/icon.png?ref=main",
                "https://api.github.com/repos/fixture-owner/fixture-repository/contents/icon.png?ref=main&redirect=1"
            })
                Require(await artwork.LoadAsync(privateApp, bad) is null, "Unsafe private image URL unexpectedly returned artwork.");
            Require(await artwork.LoadAsync(privateApp, null) is null, "A missing private artwork URL unexpectedly returned artwork.");
            var publicImage = await artwork.LoadAsync(App(isPrivate: false), IconUrl);
            Require(publicImage is { IsFrozen: true }, "The private loader broke public artwork loading.");
        }
        Require(unsafeCalls == 1, "Unsafe private URLs made requests, or the public URL failed to load.");
        Require(publicAuthorization is null, "The public artwork request received a GitHub token.");

        foreach (var status in new[] { HttpStatusCode.Unauthorized, HttpStatusCode.Forbidden, HttpStatusCode.NotFound, HttpStatusCode.Found })
        {
            var calls = 0;
            using var http = new HttpClient(new StubHandler((request, _) =>
            {
                calls++;
                var response = new HttpResponseMessage(status) { RequestMessage = request, Content = new StringContent(Token) };
                if (status == HttpStatusCode.Found) response.Headers.Location = new Uri("https://example.com/private.png");
                return Task.FromResult(response);
            }));
            Require(await new AppArtworkService(http, cache, credentials).LoadAsync(privateApp, IconUrl) is null,
                $"Private artwork HTTP status {(int)status} was not handled as an empty image.");
            Require(calls == 1, "A failed private image request was redirected or retried.");
        }

        credentials.Token = null;
        var noTokenRequests = 0;
        using (var http = new HttpClient(new StubHandler((_, _) =>
        {
            noTokenRequests++;
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK));
        })))
            Require(await new AppArtworkService(http, cache, credentials).LoadAsync(privateApp, IconUrl) is null, "Private artwork loaded without a token.");
        Require(noTokenRequests == 0, "Private artwork sent an unauthenticated request when no token was connected.");

        credentials.Token = Token;
        using (var http = new HttpClient(new StubHandler((request, _) =>
        {
            credentials.Token = "dummy-rotated-token";
            return Task.FromResult(PngResponse(request, png));
        })))
            Require(await new AppArtworkService(http, cache, credentials).LoadAsync(privateApp, IconUrl) is null,
                "Artwork fetched under a replaced token was retained.");

        var large = new byte[MaximumBytes + 1];
        using (var http = new HttpClient(new StubHandler((request, _) =>
        {
            var response = PngResponse(request, large);
            response.Content.Headers.ContentLength = large.Length;
            return Task.FromResult(response);
        })))
            Require(await new AppArtworkService(http, cache, credentials).LoadAsync(privateApp, IconUrl) is null,
                "Private artwork larger than the byte limit was accepted.");

        var started = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        using (var cancellation = new CancellationTokenSource())
        using (var http = new HttpClient(new StubHandler(async (_, token) =>
        {
            started.TrySetResult(true);
            await Task.Delay(Timeout.Infinite, token);
            return new HttpResponseMessage(HttpStatusCode.OK);
        })))
        {
            var load = new AppArtworkService(http, cache, credentials).LoadAsync(privateApp, IconUrl, cancellation.Token);
            await started.Task.WaitAsync(TimeSpan.FromSeconds(5));
            cancellation.Cancel();
            try { await load; throw new InvalidOperationException("Private artwork ignored cancellation."); }
            catch (OperationCanceledException) { }
        }

        Console.WriteLine("PASS: private GitHub artwork stays repository-scoped, authenticated, uncached, size-bounded, and cancellation-safe.");

    }

    private static AppDefinition App(bool isPrivate) => new()
    {
        Id = "artwork-fixture", Name = "Artwork fixture", Repository = Repository, IsPrivate = isPrivate
    };

    private static (Uri Uri, string? Authorization, string? Accept) Snapshot(HttpRequestMessage request) =>
        (request.RequestUri!, request.Headers.Authorization?.ToString(), string.Join(",", request.Headers.Accept.Select(value => value.MediaType)));

    private static HttpResponseMessage PngResponse(HttpRequestMessage request, byte[] bytes) =>
        new(HttpStatusCode.OK) { RequestMessage = request, Content = new ByteArrayContent(bytes) };

    private static byte[] CreateTinyPng()
    {
        var pixels = new byte[] { 0xE8, 0x87, 0xA8, 0xFF, 0x70, 0x8B, 0xFF, 0xFF,
            0x70, 0x8B, 0xFF, 0xFF, 0xE8, 0x87, 0xA8, 0xFF };
        var bitmap = BitmapSource.Create(2, 2, 96, 96, PixelFormats.Bgra32, null, pixels, 8);
        bitmap.Freeze();
        var encoder = new PngBitmapEncoder();
        encoder.Frames.Add(BitmapFrame.Create(bitmap));
        using var stream = new MemoryStream();
        encoder.Save(stream);
        return stream.ToArray();
    }

    private static void Require(bool condition, string message)
    {
        if (!condition) throw new InvalidOperationException(message);
    }

    private sealed class MutableToken(string? token) : IGitHubTokenProvider
    {
        public string? Token { get; set; } = token;
        public string? GetToken() => Token;
    }

    private sealed class StubHandler(Func<HttpRequestMessage, CancellationToken, Task<HttpResponseMessage>> send) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken) => send(request, cancellationToken);
    }
}
