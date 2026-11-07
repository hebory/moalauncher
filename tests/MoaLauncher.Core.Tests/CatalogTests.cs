using MoaLauncher.Core;

namespace MoaLauncher.Core.Tests;

public sealed class CatalogTests
{
    private static AppDefinition ValidApp(string id = "moaview") => new()
    {
        Id = id,
        Name = "모아뷰",
        Description = "이미지와 문서를 미리 봅니다.",
        Category = "파일 · 미리보기",
        AccentColor = "#8BA3FF",
        Monogram = "Mv",
        Repository = "storytold/moaview",
        Packages = [new PackageDefinition { Architecture = "x64", Kind = "inno", AssetPattern = "MoaView-*-Setup-x64.exe", ExecutablePath = "MoaView.exe", InstallDirectory = "Programs/MoaView" }]
    };

    [Fact]
    public void ValidateAcceptsAnUnpublishedPlaceholderRepository()
    {
        var app = ValidApp() with { Repository = "" };
        CatalogService.Validate(new AppCatalog { Apps = [app] });
    }

    [Fact]
    public void ValidateRejectsDuplicateIdsWithoutCaseSensitivity()
    {
        var catalog = new AppCatalog { Apps = [ValidApp("moaview"), ValidApp("MOAVIEW")] };
        Assert.Throws<InvalidDataException>(() => CatalogService.Validate(catalog));
    }

    [Theory]
    [InlineData("../outside.exe", "Programs/MoaView")]
    [InlineData("MoaView.exe", "../outside")]
    public void ValidateRejectsTraversalPaths(string executable, string installDirectory)
    {
        var app = ValidApp() with
        {
            Packages = [ValidApp().Packages[0] with { ExecutablePath = executable, InstallDirectory = installDirectory }]
        };
        Assert.Throws<InvalidDataException>(() => CatalogService.Validate(new AppCatalog { Apps = [app] }));
    }

    [Fact]
    public async Task SaveAndLoadRoundTripValidCatalog()
    {
        var folder = Path.Combine(Path.GetTempPath(), "moa-catalog-test-" + Guid.NewGuid().ToString("N"));
        try
        {
            var path = Path.Combine(folder, "apps.json");
            var expected = new AppCatalog { Apps = [ValidApp()] };
            await CatalogService.SaveAsync(path, expected);
            var actual = await CatalogService.LoadAsync(path);
            Assert.Equal(expected.Apps[0].Id, actual.Apps[0].Id);
            Assert.Equal(expected.Apps[0].Name, actual.Apps[0].Name);
            Assert.Equal(expected.Apps[0].Repository, actual.Apps[0].Repository);
            Assert.Equal(expected.Apps[0].Packages[0], actual.Apps[0].Packages[0]);
        }
        finally
        {
            if (Directory.Exists(folder)) Directory.Delete(folder, recursive: true);
        }
    }
}
