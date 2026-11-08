using System.Net;
using System.Security.Cryptography;
using MoaLauncher.Core;

namespace MoaLauncher.Core.Tests;

public sealed class SelfUpdateTests
{
    private const string Repository = "hebory/moalauncher";
    private static readonly byte[] InstallerBytes = "offline test payload; never executed"u8.ToArray();

    [Fact]
    public void DefinitionIsBoundToTheLauncherRepositoryAndExpectedInstaller()
    {
        var definition = SelfUpdateService.Definition;
        Assert.Equal("moa-launcher", definition.Id);
        Assert.Equal(Repository, definition.Repository);
        Assert.Equal("x64", Assert.Single(definition.Packages).Architecture);
        Assert.Equal("inno", definition.Packages[0].Kind);
        Assert.Equal("MoaLauncher.exe", definition.Packages[0].ExecutablePath);
        Assert.Equal("Programs/MoaLauncher", definition.Packages[0].InstallDirectory);
        Assert.Equal("MoaLauncher-*-Setup-x64.exe", definition.Packages[0].AssetPattern);
    }

    [Fact]
    public async Task DownloadsAndVerifiesASetupAssetWithoutExecutingIt()
    {
        var dataDirectory = NewDataDirectory();
        try
        {
            var release = MakeRelease("v0.2.0+build.7", directChecksum: Hash(InstallerBytes));
            var handler = new StubHandler((request, _) =>
            {
                Assert.EndsWith(release.Assets[0].Name, request.RequestUri!.AbsolutePath, StringComparison.Ordinal);
                return Task.FromResult(BytesResponse(InstallerBytes));
            });
            var prepared = await CreateService(dataDirectory, handler)
                .PrepareAsync(release, "0.1.0");

            Assert.Equal("0.2.0", prepared.Version);
            Assert.Equal(Hash(InstallerBytes), prepared.Sha256);
            Assert.True(File.Exists(prepared.InstallerPath));
            Assert.Equal(InstallerBytes, await File.ReadAllBytesAsync(prepared.InstallerPath));
            Assert.True(SafePaths.IsWithin(Path.Combine(dataDirectory, "self-update"), prepared.InstallerPath));
            Assert.EndsWith("-Setup-x64.exe", prepared.InstallerPath, StringComparison.OrdinalIgnoreCase);
        }
        finally { DeleteDataDirectory(dataDirectory); }
    }

    [Fact]
    public async Task RejectsWrongRepositoryAndNonNewerReleasesBeforeDownloading()
    {
        var dataDirectory = NewDataDirectory();
        try
        {
            var sends = 0;
            var service = CreateService(dataDirectory, new StubHandler((_, _) =>
            {
                sends++;
                return Task.FromResult(BytesResponse(InstallerBytes));
            }));

            var wrongRepository = MakeRelease("v0.2.0", directChecksum: Hash(InstallerBytes), assetRepository: "attacker/moalauncher");
            await Assert.ThrowsAsync<InvalidDataException>(() => service.PrepareAsync(wrongRepository, "0.1.0"));
            await Assert.ThrowsAsync<InvalidOperationException>(() => service.PrepareAsync(MakeRelease("v0.2.0"), "0.2.0"));
            Assert.Equal(0, sends);
            Assert.Empty(Directory.Exists(Path.Combine(dataDirectory, "self-update"))
                ? Directory.GetDirectories(Path.Combine(dataDirectory, "self-update"))
                : []);
        }
        finally { DeleteDataDirectory(dataDirectory); }
    }

    [Fact]
    public async Task RejectsChecksumMismatchAndCleansOnlyItsOwnStagingDirectory()
    {
        var dataDirectory = NewDataDirectory();
        try
        {
            var stagingRoot = Path.Combine(dataDirectory, "self-update");
            Directory.CreateDirectory(stagingRoot);
            var unrelatedPath = Path.Combine(stagingRoot, "keep-this-file.txt");
            await File.WriteAllTextAsync(unrelatedPath, "unrelated");
            var release = MakeRelease("v0.2.0", directChecksum: Hash("different bytes"u8.ToArray()));
            var service = CreateService(dataDirectory, new StubHandler((_, _) => Task.FromResult(BytesResponse(InstallerBytes))));

            var error = await Assert.ThrowsAsync<InvalidDataException>(() => service.PrepareAsync(release, "0.1.0"));

            Assert.Contains("SHA-256", error.Message);
            Assert.True(File.Exists(unrelatedPath));
            Assert.Empty(Directory.GetDirectories(stagingRoot));
            Assert.Equal([unrelatedPath], Directory.GetFiles(stagingRoot));
        }
        finally { DeleteDataDirectory(dataDirectory); }
    }

    [Fact]
    public async Task UsesChecksumsSidecarAndRetainsVerifiedInstaller()
    {
        var dataDirectory = NewDataDirectory();
        try
        {
            var release = MakeRelease("v0.2.0", includeSidecar: true);
            var handler = new StubHandler((request, _) =>
            {
                if (request.RequestUri!.AbsolutePath.EndsWith(".sha256", StringComparison.OrdinalIgnoreCase))
                {
                    var checksum = $"{Hash(InstallerBytes)}  {release.Assets[0].Name}\n";
                    return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent(checksum) });
                }
                return Task.FromResult(BytesResponse(InstallerBytes));
            });

            var prepared = await CreateService(dataDirectory, handler).PrepareAsync(release, "0.1.0");

            Assert.Equal(Hash(InstallerBytes), prepared.Sha256);
            Assert.True(File.Exists(prepared.InstallerPath));
        }
        finally { DeleteDataDirectory(dataDirectory); }
    }

    [Fact]
    public async Task CancellationRemovesItsStagingDirectoryAndLeavesUnrelatedFiles()
    {
        var dataDirectory = NewDataDirectory();
        try
        {
            var stagingRoot = Path.Combine(dataDirectory, "self-update");
            Directory.CreateDirectory(stagingRoot);
            var unrelatedPath = Path.Combine(stagingRoot, "keep-this-file.txt");
            await File.WriteAllTextAsync(unrelatedPath, "unrelated");
            using var cancellation = new CancellationTokenSource();
            var release = MakeRelease("v0.2.0", directChecksum: Hash(InstallerBytes));
            var handler = new StubHandler((_, _) =>
            {
                cancellation.Cancel();
                return Task.FromCanceled<HttpResponseMessage>(cancellation.Token);
            });

            await Assert.ThrowsAnyAsync<OperationCanceledException>(() =>
                CreateService(dataDirectory, handler).PrepareAsync(release, "0.1.0", cancellationToken: cancellation.Token));

            Assert.True(File.Exists(unrelatedPath));
            Assert.Empty(Directory.GetDirectories(stagingRoot));
            Assert.Empty(Directory.GetFiles(stagingRoot).Where(path => path != unrelatedPath));
        }
        finally { DeleteDataDirectory(dataDirectory); }
    }

    private static SelfUpdateService CreateService(string dataDirectory, HttpMessageHandler handler) =>
        new(new HttpClient(handler), dataDirectory);

    private static AppRelease MakeRelease(string tag, string? directChecksum = null,
        bool includeSidecar = false, string? assetRepository = null)
    {
        var assetName = $"MoaLauncher-{tag}-Setup-x64.exe";
        var repo = assetRepository ?? Repository;
        var assets = new List<ReleaseAsset>
        {
            new(assetName, new Uri($"https://github.com/{repo}/releases/download/{tag}/{assetName}"),
                InstallerBytes.Length, directChecksum)
        };
        if (includeSidecar)
            assets.Add(new ReleaseAsset(assetName + ".sha256",
                new Uri($"https://github.com/{Repository}/releases/download/{tag}/{assetName}.sha256"), 100, null));
        return new AppRelease(tag, $"Moa Launcher {tag}", "Test release",
            new Uri($"https://github.com/{Repository}/releases/tag/{tag}"), DateTimeOffset.UtcNow, assets);
    }

    private static HttpResponseMessage BytesResponse(byte[] bytes) =>
        new(HttpStatusCode.OK) { Content = new ByteArrayContent(bytes) };

    private static string Hash(byte[] bytes) => Convert.ToHexString(SHA256.HashData(bytes));

    private static string NewDataDirectory() => Path.Combine(Path.GetTempPath(), "moa-self-update-test-" + Guid.NewGuid().ToString("N"));

    private static void DeleteDataDirectory(string path)
    {
        var tempRoot = Path.GetFullPath(Path.GetTempPath()).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
        var full = Path.GetFullPath(path);
        if (!full.StartsWith(tempRoot + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase) ||
            !Path.GetFileName(full).StartsWith("moa-self-update-test-", StringComparison.Ordinal))
            throw new InvalidOperationException("Refusing to delete a path outside this self-update test's temporary folder.");
        if (Directory.Exists(full)) Directory.Delete(full, recursive: true);
    }

    private sealed class StubHandler(Func<HttpRequestMessage, CancellationToken, Task<HttpResponseMessage>> send) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken) =>
            send(request, cancellationToken);
    }
}
