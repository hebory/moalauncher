using System.Net;
using System.Security.Cryptography;
using System.Text.Json;
using MoaLauncher.Core;

namespace MoaLauncher.Core.Tests;

public sealed class NsisIntegrationFactAttribute : FactAttribute
{
    public NsisIntegrationFactAttribute()
    {
        if (string.IsNullOrWhiteSpace(Environment.GetEnvironmentVariable("MOALAUNCHER_NSIS_FIXTURE_ROOT")))
            Skip = "scripts/Test-NsisInstall.ps1로 NSIS 설치기 fixture를 만들었을 때만 실행합니다.";
    }
}

public sealed class NsisIntegrationTests
{
    private const string Repository = "fixture-owner/fixture-app";

    [NsisIntegrationFact]
    public async Task InstallationService_InstallsUpdatesAndRejectsBrokenNsisInstallers()
    {
        var fixtureRootValue = Environment.GetEnvironmentVariable("MOALAUNCHER_NSIS_FIXTURE_ROOT");
        Assert.False(string.IsNullOrWhiteSpace(fixtureRootValue));
        var fixtureRoot = Path.GetFullPath(fixtureRootValue!);
        Assert.True(Directory.Exists(fixtureRoot), $"NSIS fixture 폴더가 없습니다: {fixtureRoot}");

        var runRoot = Path.Combine(fixtureRoot, "test-runs", Guid.NewGuid().ToString("N"));
        var dataDirectory = Path.Combine(runRoot, "launcher data");
        var localAppData = Path.Combine(runRoot, "Local AppData 사용자");
        Directory.CreateDirectory(dataDirectory);
        Directory.CreateDirectory(localAppData);
        try
        {
            var v100 = await File.ReadAllBytesAsync(Fixture("installers", "setup-1.0.0.exe"));
            var v110 = await File.ReadAllBytesAsync(Fixture("installers", "setup-1.1.0.exe"));
            var failedInstaller = await File.ReadAllBytesAsync(Fixture("installers", "setup-failure.exe"));
            var missingExecutableInstaller = await File.ReadAllBytesAsync(Fixture("installers", "setup-missing-executable.exe"));
            var mismatchInstaller = await File.ReadAllBytesAsync(Fixture("installers", "setup-version-mismatch.exe"));
            var app = App("nsis-fixture", "Programs/nsis-fixture NSIS Integration 검증");
            using var client = new HttpClient(new FixtureHandler(new Dictionary<string, byte[]>(StringComparer.OrdinalIgnoreCase)
            {
                ["setup.exe"] = v100,
                ["setup-update.exe"] = v110,
                ["setup-failure.exe"] = failedInstaller,
                ["setup-missing-executable.exe"] = missingExecutableInstaller,
                ["setup-version-mismatch.exe"] = mismatchInstaller
            }));
            var service = new InstallationService(client, dataDirectory, localAppData);

            var first = await service.InstallAsync(app, Release("1.0.0", "setup.exe", v100));
            var expectedDirectory = Path.Combine(localAppData, "Programs", "nsis-fixture NSIS Integration 검증");
            Assert.Equal("1.0.0", first.Version);
            Assert.Equal(Path.Combine(expectedDirectory, "NsisFixture.exe"), first.ExecutablePath);
            Assert.Contains(' ', first.InstallDirectory);
            Assert.Contains(' ', first.ExecutablePath);
            Assert.Contains("사용자", first.InstallDirectory);
            Assert.Equal("1.0.0", (await service.FindInstalledAsync(app))?.Version);

            var second = await service.InstallAsync(app, Release("1.1.0", "setup-update.exe", v110));
            Assert.Equal("1.1.0", second.Version);
            Assert.Equal("1.1.0", (await service.FindInstalledAsync(app))?.Version);
            var statePath = Path.Combine(dataDirectory, "installed", app.Id + ".json");
            var stateAfterUpdate = await File.ReadAllTextAsync(statePath);

            var failure = await Assert.ThrowsAsync<InvalidOperationException>(() =>
                service.InstallAsync(app, Release("1.2.0", "setup-failure.exe", failedInstaller)));
            Assert.Contains("오류 코드 42", failure.Message);
            Assert.Equal(stateAfterUpdate, await File.ReadAllTextAsync(statePath));

            var mismatch = await Assert.ThrowsAsync<InvalidDataException>(() =>
                service.InstallAsync(app, Release("1.2.0", "setup-version-mismatch.exe", mismatchInstaller)));
            Assert.Contains("릴리스 버전(1.2.0)", mismatch.Message);
            Assert.Equal(stateAfterUpdate, await File.ReadAllTextAsync(statePath));
            var actualAfterMismatch = JsonSerializer.Deserialize<InstalledApp>(
                await File.ReadAllTextAsync(statePath), CatalogService.JsonOptions);
            Assert.Equal("1.1.0", actualAfterMismatch?.Version);

            var missingApp = App("nsis-missing-executable", "Programs/nsis-missing-executable NSIS Integration 검증");
            var missing = await Assert.ThrowsAsync<InvalidDataException>(() =>
                service.InstallAsync(missingApp, Release("1.1.0", "setup-missing-executable.exe", missingExecutableInstaller)));
            Assert.Contains("실행 파일을 찾을 수 없습니다", missing.Message);
            Assert.Null(await service.FindInstalledAsync(missingApp));
            Assert.False(File.Exists(Path.Combine(dataDirectory, "installed", missingApp.Id + ".json")));
        }
        finally
        {
            if (Directory.Exists(runRoot)) Directory.Delete(runRoot, recursive: true);
        }

        string Fixture(params string[] parts) => Path.Combine([fixtureRoot, .. parts]);
    }

    private static AppDefinition App(string id, string installDirectory) => new()
    {
        Id = id,
        Name = "NSIS Fixture",
        Repository = Repository,
        Packages =
        [
            new PackageDefinition
            {
                Architecture = "x64",
                Kind = "nsis",
                AssetPattern = "setup*.exe",
                ExecutablePath = "NsisFixture.exe",
                InstallDirectory = installDirectory
            }
        ]
    };

    private static AppRelease Release(string version, string assetName, byte[] contents)
    {
        var tag = "v" + version;
        var uri = new Uri($"https://github.com/{Repository}/releases/download/{tag}/{assetName}");
        return new AppRelease(tag, tag, "", new Uri($"https://github.com/{Repository}/releases/tag/{tag}"),
            DateTimeOffset.UtcNow,
            [new ReleaseAsset(assetName, uri, contents.Length, Convert.ToHexString(SHA256.HashData(contents))) ]);
    }

    private sealed class FixtureHandler(IReadOnlyDictionary<string, byte[]> fixtures) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var name = Path.GetFileName(request.RequestUri?.AbsolutePath);
            if (name is null || !fixtures.TryGetValue(name, out var bytes))
                return Task.FromResult(new HttpResponseMessage(HttpStatusCode.NotFound) { RequestMessage = request });
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK)
            {
                RequestMessage = request,
                Content = new ByteArrayContent(bytes)
            });
        }
    }
}
