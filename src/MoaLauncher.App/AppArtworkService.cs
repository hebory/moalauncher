using System.IO;
using System.Net;
using System.Net.Http;
using System.Security.Cryptography;
using System.Text;
using System.Windows.Media.Imaging;

namespace MoaLauncher.App;

internal sealed class AppArtworkService(HttpClient http, string cacheDirectory)
{
    private const int MaximumBytes = 8 * 1024 * 1024;
    private const int MaximumDimension = 8192;
    private const long MaximumPixels = 32_000_000;
    private static readonly TimeSpan RequestTimeout = TimeSpan.FromSeconds(10);
    private readonly HttpClient _http = http ?? throw new ArgumentNullException(nameof(http));
    private readonly string _cacheDirectory = Path.GetFullPath(cacheDirectory ?? throw new ArgumentNullException(nameof(cacheDirectory)));

    internal async Task<BitmapSource?> LoadAsync(string? url, CancellationToken cancellationToken = default)
    {
        cancellationToken.ThrowIfCancellationRequested();
        if (!TryGetArtworkUri(url, out var uri)) return null;

        var cachePath = GetCachePath(url!);
        BitmapSource? cachedImage = null;
        try
        {
            if (File.Exists(cachePath))
            {
                var cachedBytes = await ReadBoundedFileAsync(cachePath, cancellationToken).ConfigureAwait(false);
                if (cachedBytes is not null)
                    cachedImage = await Task.Run(() => DecodeImage(cachedBytes), cancellationToken).ConfigureAwait(false);
                cancellationToken.ThrowIfCancellationRequested();
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested) { throw; }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or InvalidDataException or NotSupportedException or ArgumentException) { }

        try
        {
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
            timeout.CancelAfter(RequestTimeout);
            using var request = new HttpRequestMessage(HttpMethod.Get, uri);
            using var response = await _http.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, timeout.Token).ConfigureAwait(false);
            if (response.StatusCode != HttpStatusCode.OK || response.RequestMessage?.RequestUri is not { } finalUri || !uri.Equals(finalUri))
                return cachedImage;
            if (response.Content.Headers.ContentLength is > MaximumBytes) return cachedImage;

            await using var stream = await response.Content.ReadAsStreamAsync(timeout.Token).ConfigureAwait(false);
            var downloadedBytes = await ReadBoundedAsync(stream, timeout.Token).ConfigureAwait(false);
            if (downloadedBytes is null) return cachedImage;

            BitmapSource? downloadedImage;
            try
            {
                downloadedImage = await Task.Run(() => DecodeImage(downloadedBytes), cancellationToken).ConfigureAwait(false);
                cancellationToken.ThrowIfCancellationRequested();
            }
            catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested) { throw; }
            catch (Exception ex) when (ex is IOException or InvalidDataException or FormatException or NotSupportedException or ArgumentException or InvalidOperationException or System.Runtime.InteropServices.COMException)
            {
                return cachedImage;
            }
            if (downloadedImage is null) return cachedImage;

            await TryWriteCacheAsync(cachePath, downloadedBytes, cancellationToken).ConfigureAwait(false);
            return downloadedImage;
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested) { throw; }
        catch (Exception ex) when (ex is HttpRequestException or IOException or UnauthorizedAccessException or InvalidDataException or NotSupportedException or TaskCanceledException or ObjectDisposedException)
        {
            return cachedImage;
        }
    }

    private static bool TryGetArtworkUri(string? value, out Uri uri)
    {
        uri = null!;
        if (string.IsNullOrWhiteSpace(value) || !Uri.TryCreate(value, UriKind.Absolute, out var parsed) ||
            !string.Equals(parsed.Scheme, Uri.UriSchemeHttps, StringComparison.OrdinalIgnoreCase) ||
            !string.Equals(parsed.IdnHost, "raw.githubusercontent.com", StringComparison.OrdinalIgnoreCase) ||
            !parsed.IsDefaultPort || parsed.UserInfo.Length != 0 || parsed.Query.Length != 0 || parsed.Fragment.Length != 0)
            return false;
        uri = parsed;
        return true;
    }

    private string GetCachePath(string originalUrl)
    {
        var hash = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(originalUrl))).ToLowerInvariant();
        var path = Path.GetFullPath(Path.Combine(_cacheDirectory, hash + ".image"));
        var root = _cacheDirectory.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar) + Path.DirectorySeparatorChar;
        if (!path.StartsWith(root, StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException("Artwork cache path escaped its directory.");
        return path;
    }

    private static async Task<byte[]?> ReadBoundedFileAsync(string path, CancellationToken cancellationToken)
    {
        await using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read, 81920, useAsync: true);
        if (stream.Length > MaximumBytes) return null;
        return await ReadBoundedAsync(stream, cancellationToken).ConfigureAwait(false);
    }

    private static async Task<byte[]?> ReadBoundedAsync(Stream stream, CancellationToken cancellationToken)
    {
        using var buffer = new MemoryStream();
        var chunk = new byte[81920];
        while (true)
        {
            var read = await stream.ReadAsync(chunk, cancellationToken).ConfigureAwait(false);
            if (read == 0) break;
            if (buffer.Length + read > MaximumBytes) return null;
            buffer.Write(chunk, 0, read);
        }
        return buffer.ToArray();
    }

    private static BitmapSource? DecodeImage(byte[] bytes)
    {
        if (bytes.Length == 0 || bytes.Length > MaximumBytes) return null;

        try
        {
            using var stream = new MemoryStream(bytes, writable: false);
            var decoder = BitmapDecoder.Create(stream, BitmapCreateOptions.DelayCreation, BitmapCacheOption.None);
            if (decoder.Frames.Count == 0) return null;
            var frame = decoder.Frames[0];
            if (decoder is not (PngBitmapDecoder or JpegBitmapDecoder))
                return null;

            var width = frame.PixelWidth;
            var height = frame.PixelHeight;
            if (width <= 0 || height <= 0 || width > MaximumDimension || height > MaximumDimension ||
                (long)width * height > MaximumPixels)
                return null;

            using var imageStream = new MemoryStream(bytes, writable: false);
            var image = new BitmapImage();
            image.BeginInit();
            image.CacheOption = BitmapCacheOption.OnLoad;
            image.DecodePixelWidth = Math.Min(width, 1200);
            image.StreamSource = imageStream;
            image.EndInit();
            image.Freeze();
            return image;
        }
        catch (Exception ex) when (ex is IOException or InvalidDataException or FormatException or NotSupportedException or ArgumentException or InvalidOperationException or System.Runtime.InteropServices.COMException)
        {
            return null;
        }
    }

    private async Task TryWriteCacheAsync(string path, byte[] bytes, CancellationToken cancellationToken)
    {
        string? temporaryPath = null;
        try
        {
            Directory.CreateDirectory(_cacheDirectory);
            temporaryPath = path + ".tmp-" + Guid.NewGuid().ToString("N");
            await File.WriteAllBytesAsync(temporaryPath, bytes, cancellationToken).ConfigureAwait(false);
            File.Move(temporaryPath, path, overwrite: true);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested) { throw; }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or ArgumentException or NotSupportedException) { }
        finally
        {
            if (temporaryPath is not null)
            {
                try { if (File.Exists(temporaryPath)) File.Delete(temporaryPath); }
                catch (IOException) { }
                catch (UnauthorizedAccessException) { }
            }
        }
    }
}
