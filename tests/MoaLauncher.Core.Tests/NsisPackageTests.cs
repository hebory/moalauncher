using System.Diagnostics;
using MoaLauncher.Core;

namespace MoaLauncher.Core.Tests;

public sealed class NsisPackageTests
{
    private static AppDefinition App(PackageDefinition? package = null) => new()
    {
        Id = "private-tool", Name = "Private Tool", Repository = "owner/private-tool", IsPrivate = true,
        Packages = [package ?? Package()]
    };

    private static PackageDefinition Package() => new()
    {
        Kind = "nsis", Architecture = "x64", AssetPattern = "Tool_*-setup.exe",
        ExecutablePath = "tool.exe", InstallDirectory = "Programs/개인 도구"
    };

    [Fact]
    public void PrivateNsisCatalogIsValidButCannotBeBundled()
    {
        var catalog = new AppCatalog { Apps = [App()] };
        CatalogService.Validate(catalog);
        Assert.Throws<InvalidDataException>(() => CatalogService.ValidateBundled(catalog));
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("../outside")]
    [InlineData("C:/outside")]
    public void NsisRequiresSafeLocalInstallDirectory(string? directory)
    {
        Assert.Throws<InvalidDataException>(() => CatalogService.Validate(new AppCatalog
        {
            Apps = [App(Package() with { InstallDirectory = directory })]
        }));
    }

    [Fact]
    public void NsisRejectsZipAssetPattern()
    {
        Assert.Throws<InvalidDataException>(() => CatalogService.Validate(new AppCatalog
        {
            Apps = [App(Package() with { AssetPattern = "Tool_*.zip" })]
        }));
    }

    [Fact]
    public void NsisDirectoryWithSpacesIsLastAndUnquotedWithoutShell()
    {
        var directory = Path.GetFullPath(Path.Combine("test data", "한글 도구"));
        var installer = Path.GetFullPath(Path.Combine("downloads", "setup.exe"));
        var start = InstallationService.CreateInstallerStartInfo("nsis", installer, directory);
        Assert.Equal(installer, start.FileName);
        Assert.Equal(Path.GetDirectoryName(installer), start.WorkingDirectory);
        Assert.Equal("/S /D=" + directory, start.Arguments);
        Assert.Empty(start.ArgumentList);
        Assert.False(start.UseShellExecute);
    }

    [Theory]
    [InlineData("relative/path")]
    [InlineData("/bad\"path")]
    [InlineData("/bad\npath")]
    public void NsisRejectsAmbiguousCommandLinePaths(string path)
    {
        Assert.Throws<InvalidDataException>(() => InstallationService.CreateInstallerStartInfo("nsis", "setup.exe", path));
    }

    [Fact]
    public void InnoKeepsItsOwnArguments()
    {
        var directory = Path.GetFullPath("test data");
        var start = InstallationService.CreateInstallerStartInfo("inno", "setup.exe", directory);
        Assert.Empty(start.Arguments);
        Assert.Contains("/NORESTART", start.ArgumentList);
        Assert.Contains("/SILENT", start.ArgumentList);
        Assert.Contains("/DIR=" + directory, start.ArgumentList);
        Assert.DoesNotContain("/S", start.ArgumentList);
    }

    [Fact]
    public async Task FindsExistingNsisInstallFromActualFileVersionWithoutState()
    {
        var root = Path.Combine(Path.GetTempPath(), "moa-nsis-" + Guid.NewGuid().ToString("N"));
        try
        {
            var app = App();
            var executable = SafePaths.ResolveRelative(SafePaths.ResolveRelative(root, app.Packages[0].InstallDirectory!), "tool.exe");
            Directory.CreateDirectory(Path.GetDirectoryName(executable)!);
            File.Copy(typeof(InstallationService).Assembly.Location, executable);
            using var client = new HttpClient();
            var service = new InstallationService(client, Path.Combine(root, "state"), root);
            var installed = await service.FindInstalledAsync(app);
            Assert.NotNull(installed);
            Assert.Equal("nsis", installed.PackageKind);
            Assert.Equal(FileVersionInfo.GetVersionInfo(executable).ProductVersion!.Split('+')[0], installed.Version);
            Assert.Equal(executable, installed.ExecutablePath);
            Assert.False(Directory.Exists(Path.Combine(root, "state")));
        }
        finally
        {
            if (Directory.Exists(root)) Directory.Delete(root, recursive: true);
        }
    }
}
