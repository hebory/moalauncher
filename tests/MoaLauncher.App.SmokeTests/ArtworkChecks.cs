using System.IO;
using System.Net;
using System.Net.Http;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using MoaLauncher.App;
using MoaLauncher.Core;

namespace MoaLauncher.App.SmokeTests;

internal static class ArtworkChecks
{
    private const string IconUrl = "https://raw.githubusercontent.com/hebory/moaview/main/icon.png";
    private const string CoverUrl = "https://raw.githubusercontent.com/hebory/moaview/main/cover.png";
    private const int MaximumArtworkBytes = 8 * 1024 * 1024;

    public static async Task RunAsync(string dataDirectory)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(dataDirectory);
        var cacheDirectory = Path.GetFullPath(Path.Combine(dataDirectory, "artwork-smoke-cache"));
        Directory.CreateDirectory(cacheDirectory);
        var png = CreateTinyPng();

        var successfulRequests = 0;
        using (var http = new HttpClient(new StubHandler((request, _) =>
        {
            successfulRequests++;
            return Task.FromResult(PngResponse(request, png));
        })))
        {
            var image = await new AppArtworkService(http, cacheDirectory).LoadAsync(IconUrl);
            Require(image is not null && image.IsFrozen, "A valid artwork PNG was not decoded into a frozen BitmapSource.");
            Require(image!.PixelWidth == 2 && image.PixelHeight == 2, "The decoded PNG dimensions did not match the fixture.");
        }
        Require(successfulRequests == 1, "The valid icon URL did not make exactly one request.");

        var offlineRequests = 0;
        using (var http = new HttpClient(new StubHandler((request, _) =>
        {
            offlineRequests++;
            throw new HttpRequestException("offline smoke fixture");
        })))
        {
            var cachedImage = await new AppArtworkService(http, cacheDirectory).LoadAsync(IconUrl);
            Require(cachedImage is not null && cachedImage.IsFrozen,
                "The cached artwork was not returned when the network was offline.");
        }
        Require(offlineRequests == 1, "The offline cache fallback did not perform one conditional network attempt.");

        var invalidUrlRequests = 0;
        using (var http = new HttpClient(new StubHandler((_, _) =>
        {
            invalidUrlRequests++;
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK));
        })))
        {
            var service = new AppArtworkService(http, cacheDirectory);
            Require(await service.LoadAsync(null) is null, "A null artwork URL unexpectedly returned an image.");
            Require(await service.LoadAsync("https://example.com/image.png") is null,
                "An untrusted artwork host unexpectedly returned an image.");
            Require(await service.LoadAsync("http://raw.githubusercontent.com/hebory/moaview/main/icon.png") is null,
                "An insecure artwork URL unexpectedly returned an image.");
        }
        Require(invalidUrlRequests == 0, "An invalid or untrusted artwork URL made an HTTP request.");

        var damagedUrl = "https://raw.githubusercontent.com/hebory/moaview/main/damaged-image.png";
        using (var http = new HttpClient(new StubHandler((request, _) =>
            Task.FromResult(PngResponse(request, "not a png"u8.ToArray())))))
        {
            Require(await new AppArtworkService(http, cacheDirectory).LoadAsync(damagedUrl) is null,
                "Corrupt artwork bytes unexpectedly decoded into an image.");
        }

        var oversizedUrl = "https://raw.githubusercontent.com/hebory/moaview/main/oversized-image.png";
        using (var http = new HttpClient(new StubHandler((request, _) =>
        {
            var response = PngResponse(request, png);
            response.Content.Headers.ContentLength = MaximumArtworkBytes + 1L;
            return Task.FromResult(response);
        })))
        {
            Require(await new AppArtworkService(http, cacheDirectory).LoadAsync(oversizedUrl) is null,
                "Artwork larger than the configured byte limit unexpectedly decoded.");
        }

        var cancellationStarted = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        using (var cancellation = new CancellationTokenSource())
        using (var http = new HttpClient(new StubHandler(async (_, requestToken) =>
        {
            cancellationStarted.TrySetResult(true);
            await Task.Delay(Timeout.Infinite, requestToken);
            throw new InvalidOperationException("The cancellation fixture unexpectedly completed.");
        })))
        {
            var load = new AppArtworkService(http, cacheDirectory).LoadAsync(CoverUrl, cancellation.Token);
            await cancellationStarted.Task.WaitAsync(TimeSpan.FromSeconds(5));
            cancellation.Cancel();
            try
            {
                await load;
                throw new InvalidOperationException("Artwork loading ignored its cancellation token.");
            }
            catch (OperationCanceledException) { }
        }

        CheckAppItemBindings(png);
        Console.WriteLine("PASS: artwork decoding, frozen bitmaps, cache fallback, URL filtering, size limits, and cancellation.");
    }

    private static void CheckAppItemBindings(byte[] png)
    {
        var image = DecodeFixture(png);
        var item = new AppItem(new AppDefinition { Id = "artwork-smoke", Name = "Artwork smoke" });
        var notifications = new List<string?>();
        item.PropertyChanged += (_, args) => notifications.Add(args.PropertyName);

        item.IconImage = image;
        item.CoverImage = image;
        Require(item.HasCover, "Setting CoverImage did not update HasCover.");
        Require(notifications.Contains(nameof(AppItem.IconImage)) &&
                notifications.Contains(nameof(AppItem.CoverImage)) &&
                notifications.Contains(nameof(AppItem.HasCover)),
            "Artwork properties did not notify their bound UI properties.");

        item.CoverImage = null;
        Require(!item.HasCover, "Clearing CoverImage did not clear HasCover.");
    }

    private static byte[] CreateTinyPng()
    {
        var pixels = new byte[]
        {
            0xE8, 0x87, 0xA8, 0xFF, 0x70, 0x8B, 0xFF, 0xFF,
            0x70, 0x8B, 0xFF, 0xFF, 0xE8, 0x87, 0xA8, 0xFF
        };
        var bitmap = BitmapSource.Create(2, 2, 96, 96, PixelFormats.Bgra32, null, pixels, 8);
        bitmap.Freeze();
        var encoder = new PngBitmapEncoder();
        encoder.Frames.Add(BitmapFrame.Create(bitmap));
        using var stream = new MemoryStream();
        encoder.Save(stream);
        return stream.ToArray();
    }

    private static BitmapSource DecodeFixture(byte[] png)
    {
        using var stream = new MemoryStream(png, writable: false);
        var decoder = new PngBitmapDecoder(stream, BitmapCreateOptions.PreservePixelFormat, BitmapCacheOption.OnLoad);
        var image = decoder.Frames[0];
        image.Freeze();
        return image;
    }

    private static HttpResponseMessage PngResponse(HttpRequestMessage request, byte[] bytes) =>
        new(HttpStatusCode.OK)
        {
            RequestMessage = request,
            Content = new ByteArrayContent(bytes)
        };

    private static void Require(bool condition, string message)
    {
        if (!condition) throw new InvalidOperationException(message);
    }

    private sealed class StubHandler(Func<HttpRequestMessage, CancellationToken, Task<HttpResponseMessage>> send) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken) =>
            send(request, cancellationToken);
    }
}
