using System.IO.Compression;
using System.Security.Cryptography;
using System.Text;
using MoaLauncher.Core;

namespace MoaLauncher.Core.Tests;

public sealed class InstallationTests
{
    [Fact]
    public async Task InstallZip_VerifiesAndRecordsVersionAtomically()
    {
        using var temp = new TemporaryDirectory();
        var archive = CreateZip(("MoaView.exe", Encoding.UTF8.GetBytes("executable")));
        var app = ZipApp();
        var release = Release("v1.2.3", archive);
        var service = Service(temp.Path, archive);

        var installed = await service.InstallAsync(app, release);

        Assert.Equal("v1.2.3", installed.Version);
        Assert.Equal("executable", await File.ReadAllTextAsync(installed.ExecutablePath));
        Assert.Equal(installed, await service.FindInstalledAsync(app));
        Assert.Empty(Directory.GetFiles(Path.Combine(temp.Path, "downloads", "test-app")));
        Assert.Empty(Directory.GetDirectories(Path.Combine(temp.Path, "apps", "test-app"), ".staging-*"));
    }

    [Fact]
    public async Task InstallZip_AcceptsMatchingSidecarChecksum()
    {
        using var temp = new TemporaryDirectory();
        var archive = CreateZip(("MoaView.exe", Encoding.UTF8.GetBytes("sidecar")));
        var checksum = Encoding.ASCII.GetBytes(Convert.ToHexString(SHA256.HashData(archive)).ToLowerInvariant() + "  app.zip\n");
        var release = new AppRelease("v1.2.3", "", "", new Uri("https://github.com/owner/repo/releases/tag/v1.2.3"), DateTimeOffset.UtcNow,
        [
            new ReleaseAsset("app.zip", new Uri("https://github.com/owner/repo/releases/download/v1.2.3/app.zip"), archive.Length, null),
            new ReleaseAsset("app.zip.sha256", new Uri("https://github.com/owner/repo/releases/download/v1.2.3/app.zip.sha256"), checksum.Length, null)
        ]);
        using var client = new HttpClient(new StubHandler(request => new HttpResponseMessage(System.Net.HttpStatusCode.OK)
        {
            RequestMessage = request,
            Content = new ByteArrayContent(request.RequestUri!.AbsolutePath.EndsWith(".sha256", StringComparison.Ordinal) ? checksum : archive)
        }));
        var service = new InstallationService(client, temp.Path, Path.Combine(temp.Path, "local"));

        var installed = await service.InstallAsync(ZipApp(), release);

        Assert.Equal("sidecar", await File.ReadAllTextAsync(installed.ExecutablePath));
    }

    [Fact]
    public async Task InstallZip_ChecksumMismatchPreservesPreviousInstalledState()
    {
        using var temp = new TemporaryDirectory();
        var archive = CreateZip(("MoaView.exe", Encoding.UTF8.GetBytes("v1")));
        var app = ZipApp();
        var service = Service(temp.Path, archive);
        var previous = await service.InstallAsync(app, Release("v1.0.0", archive));
        var bad = Release("v2.0.0", archive, sha256: new string('0', 64));

        await Assert.ThrowsAsync<InvalidDataException>(() => service.InstallAsync(app, bad));

        Assert.Equal(previous, await service.FindInstalledAsync(app));
        Assert.False(Directory.Exists(Path.Combine(temp.Path, "apps", "test-app", "2.0.0")));
    }

    [Fact]
    public async Task InstallCancelledBeforeDownloadLeavesNoStateOrStagingFiles()
    {
        using var temp = new TemporaryDirectory();
        var archive = CreateZip(("MoaView.exe", Encoding.UTF8.GetBytes("executable")));
        using var cancellation = new CancellationTokenSource();
        cancellation.Cancel();
        var service = Service(temp.Path, archive);

        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => service.InstallAsync(
            ZipApp(), Release("v1.0.0", archive), cancellationToken: cancellation.Token));

        Assert.False(Directory.Exists(Path.Combine(temp.Path, "downloads")));
        Assert.False(Directory.Exists(Path.Combine(temp.Path, "apps")));
        Assert.False(Directory.Exists(Path.Combine(temp.Path, "installed")));
    }

    [Fact]
    public async Task FailedExtractionCanRetrySameVersion()
    {
        using var temp = new TemporaryDirectory();
        var app = ZipApp();
        var invalid = CreateZip(("../outside.txt", Encoding.UTF8.GetBytes("unsafe")));
        var service = Service(temp.Path, invalid);
        await Assert.ThrowsAsync<InvalidDataException>(() => service.InstallAsync(app, Release("v1.0.0", invalid)));

        var valid = CreateZip(("MoaView.exe", Encoding.UTF8.GetBytes("valid")));
        using var retryClient = new HttpClient(new StubHandler(request => new HttpResponseMessage(System.Net.HttpStatusCode.OK)
        {
            RequestMessage = request,
            Content = new ByteArrayContent(valid)
        }));
        var retryService = new InstallationService(retryClient, temp.Path, Path.Combine(temp.Path, "local"));
        var installed = await retryService.InstallAsync(app, Release("v1.0.0", valid));

        Assert.Equal("valid", await File.ReadAllTextAsync(installed.ExecutablePath));
        Assert.Equal("v1.0.0", (await retryService.FindInstalledAsync(app))?.Version);
    }

    [Fact]
    public async Task InstallZip_RejectsPackageWithoutPublishedChecksum()
    {
        using var temp = new TemporaryDirectory();
        var archive = CreateZip(("MoaView.exe", Encoding.UTF8.GetBytes("executable")));
        var service = Service(temp.Path, archive);
        var release = Release("v1.0.0", archive, includeSha256: false);

        await Assert.ThrowsAsync<InvalidDataException>(() => service.InstallAsync(ZipApp(), release));

        Assert.False(Directory.Exists(Path.Combine(temp.Path, "installed")));
    }

    [Theory]
    [InlineData("../outside.txt")]
    [InlineData("C:/outside.txt")]
    [InlineData("folder/CON.txt")]
    [InlineData("folder/trailing. ")]
    public async Task InstallZip_RejectsUnsafeArchivePath(string entryName)
    {
        using var temp = new TemporaryDirectory();
        var archive = CreateZip(("MoaView.exe", Encoding.UTF8.GetBytes("executable")), (entryName, Encoding.UTF8.GetBytes("unsafe")));
        var service = Service(temp.Path, archive);

        await Assert.ThrowsAsync<InvalidDataException>(() => service.InstallAsync(ZipApp(), Release("v1.0.0", archive)));

        Assert.False(File.Exists(Path.Combine(temp.Path, "outside.txt")));
        Assert.False(Directory.Exists(Path.Combine(temp.Path, "installed")));
        Assert.Empty(Directory.GetDirectories(Path.Combine(temp.Path, "apps", "test-app"), ".staging-*"));
    }

    [Fact]
    public async Task InstallZip_RejectsUnixSymbolicLink()
    {
        using var temp = new TemporaryDirectory();
        var archive = CreateZip(new[] { ("MoaView.exe", Encoding.UTF8.GetBytes("executable")), ("link", Encoding.UTF8.GetBytes("target")) }, symbolLinkName: "link");

        await Assert.ThrowsAsync<InvalidDataException>(() => Service(temp.Path, archive).InstallAsync(ZipApp(), Release("v1.0.0", archive)));

        Assert.False(Directory.Exists(Path.Combine(temp.Path, "installed")));
    }

    [Fact]
    public async Task InstallZip_RejectsExcessiveCompressionRatio()
    {
        using var temp = new TemporaryDirectory();
        var highlyCompressed = CreateZip(("MoaView.exe", Enumerable.Repeat((byte)'A', 8_000_000).ToArray()));

        await Assert.ThrowsAsync<InvalidDataException>(() => Service(temp.Path, highlyCompressed).InstallAsync(ZipApp(), Release("v1.0.0", highlyCompressed)));

        Assert.False(Directory.Exists(Path.Combine(temp.Path, "installed")));
    }

    [Fact]
    public async Task Downloader_RejectsRedirectOutsideGithubReleaseHosts()
    {
        using var temp = new TemporaryDirectory();
        var initial = new Uri("https://github.com/example/repo/releases/download/v1/app.zip");
        var final = new Uri("https://attacker.example/payload.zip");
        using var client = new HttpClient(new StubHandler(_ => new HttpResponseMessage(System.Net.HttpStatusCode.OK)
        {
            RequestMessage = new HttpRequestMessage(HttpMethod.Get, final),
            Content = new ByteArrayContent([1, 2, 3])
        }));

        await Assert.ThrowsAsync<InvalidDataException>(() => new VerifiedDownloader(client).DownloadAsync(initial, Path.Combine(temp.Path, "package.zip")));

        Assert.False(File.Exists(Path.Combine(temp.Path, "package.zip")));
    }

    [Fact]
    public async Task Downloader_ValidatesEveryAllowedRedirectHop()
    {
        using var temp = new TemporaryDirectory();
        var requests = new List<Uri>();
        using var client = new HttpClient(new StubHandler(request =>
        {
            requests.Add(request.RequestUri!);
            if (requests.Count == 1)
            {
                var redirect = new HttpResponseMessage(System.Net.HttpStatusCode.Redirect);
                redirect.Headers.Location = new Uri("https://release-assets.githubusercontent.com/release/app.zip");
                redirect.RequestMessage = request;
                return redirect;
            }
            return new HttpResponseMessage(System.Net.HttpStatusCode.OK)
            {
                RequestMessage = request,
                Content = new ByteArrayContent([1, 2, 3])
            };
        }));

        var result = await new VerifiedDownloader(client).DownloadAsync(
            new Uri("https://github.com/example/repo/releases/download/v1/app.zip"), Path.Combine(temp.Path, "package.zip"));

        Assert.Equal(2, requests.Count);
        Assert.All(requests, uri => Assert.True(VerifiedDownloader.IsAllowedReleaseUri(uri)));
        Assert.Equal(3, result.Bytes);
    }

    [Fact]
    public async Task Downloader_SizeLimitFailureLeavesNoPartialFile()
    {
        using var temp = new TemporaryDirectory();
        using var client = new HttpClient(new StubHandler(request => new HttpResponseMessage(System.Net.HttpStatusCode.OK)
        {
            RequestMessage = request,
            Content = new ByteArrayContent(Enumerable.Repeat((byte)7, 8).ToArray())
        }));
        var destination = Path.Combine(temp.Path, "package.zip");

        await Assert.ThrowsAsync<InvalidDataException>(() => new VerifiedDownloader(client).DownloadAsync(
            new Uri("https://github.com/example/repo/releases/download/v1/app.zip"), destination, maximumBytes: 4));

        Assert.False(File.Exists(destination));
        Assert.Empty(Directory.GetFiles(temp.Path));
    }

    private static InstallationService Service(string dataDirectory, byte[] package)
    {
        var handler = new StubHandler(request =>
        {
            if (request.RequestUri?.AbsolutePath.EndsWith("app.zip", StringComparison.Ordinal) != true)
                return new HttpResponseMessage(System.Net.HttpStatusCode.NotFound);
            return new HttpResponseMessage(System.Net.HttpStatusCode.OK)
            {
                RequestMessage = request,
                Content = new ByteArrayContent(package)
            };
        });
        return new InstallationService(new HttpClient(handler), dataDirectory, Path.Combine(dataDirectory, "local"));
    }

    private static AppDefinition ZipApp() => new()
    {
        Id = "test-app",
        Name = "Test App",
        Repository = "owner/repo",
        Packages = [new PackageDefinition { Architecture = "x64", Kind = "zip", AssetPattern = "app.zip", ExecutablePath = "MoaView.exe" }]
    };

    private static AppRelease Release(string tag, byte[] package, string? sha256 = null, bool includeSha256 = true)
    {
        var hash = sha256 ?? Convert.ToHexString(SHA256.HashData(package));
        return new AppRelease(tag, tag, "", new Uri("https://github.com/owner/repo/releases/tag/" + tag), DateTimeOffset.UtcNow,
            [new ReleaseAsset("app.zip", new Uri("https://github.com/owner/repo/releases/download/" + tag + "/app.zip"), package.Length, includeSha256 ? hash : null)]);
    }

    private static byte[] CreateZip(params (string Name, byte[] Contents)[] files) => CreateZip(files, symbolLinkName: null);

    private static byte[] CreateZip((string Name, byte[] Contents)[] files, string? symbolLinkName)
    {
        using var stream = new MemoryStream();
        using (var archive = new ZipArchive(stream, ZipArchiveMode.Create, leaveOpen: true))
        {
            foreach (var (name, contents) in files)
            {
                var entry = archive.CreateEntry(name, CompressionLevel.Optimal);
                if (name == symbolLinkName) entry.ExternalAttributes = unchecked((int)0xA0000000);
                using var output = entry.Open();
                output.Write(contents);
            }
        }
        return stream.ToArray();
    }

    private sealed class StubHandler(Func<HttpRequestMessage, HttpResponseMessage> responseFactory) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken) =>
            Task.FromResult(responseFactory(request));
    }

    private sealed class TemporaryDirectory : IDisposable
    {
        public string Path { get; } = System.IO.Path.Combine(System.IO.Path.GetTempPath(), "moalauncher-tests-" + Guid.NewGuid().ToString("N"));

        public TemporaryDirectory() => Directory.CreateDirectory(Path);

        public void Dispose()
        {
            if (Directory.Exists(Path)) Directory.Delete(Path, recursive: true);
        }
    }
}
