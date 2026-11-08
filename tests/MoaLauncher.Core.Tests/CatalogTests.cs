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
        IconUrl = "https://raw.githubusercontent.com/storytold/moaview/main/icon.png",
        CoverUrl = "https://raw.githubusercontent.com/storytold/moaview/main/cover.jpg",
        Packages = [new PackageDefinition { Architecture = "x64", Kind = "inno", AssetPattern = "MoaView-*-Setup-x64.exe", ExecutablePath = "MoaView.exe", InstallDirectory = "Programs/MoaView" }]
    };

    [Fact]
    public async Task LoadAcceptsLegacyCatalogWithoutOptionalImageUrls()
    {
        var folder = Path.Combine(Path.GetTempPath(), "moa-catalog-legacy-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(folder);
        try
        {
            var path = Path.Combine(folder, "apps.json");
            await File.WriteAllTextAsync(path, """
                {"schemaVersion":1,"apps":[{"id":"old-app","name":"Old","repository":"example/old","packages":[{"architecture":"x64","kind":"zip","assetPattern":"Old-*.zip","executablePath":"Old.exe"}]}]}
                """);
            var loaded = await CatalogService.LoadAsync(path);
            Assert.Null(loaded.Apps[0].IconUrl);
            Assert.Null(loaded.Apps[0].CoverUrl);
        }
        finally
        {
            if (Directory.Exists(folder)) Directory.Delete(folder, recursive: true);
        }
    }

    [Fact]
    public void ValidateAcceptsOptionalRepositoryImages()
    {
        CatalogService.Validate(new AppCatalog { Apps = [ValidApp()] });
    }

    [Theory]
    [InlineData("http://raw.githubusercontent.com/storytold/moaview/main/icon.png")]
    [InlineData("https://example.com/storytold/moaview/main/icon.png")]
    [InlineData("https://raw.githubusercontent.com/other/moaview/main/icon.png")]
    [InlineData("https://user@raw.githubusercontent.com/storytold/moaview/main/icon.png")]
    [InlineData("https://raw.githubusercontent.com/storytold/moaview/main/icon.svg")]
    [InlineData("https://raw.githubusercontent.com/storytold/moaview/main/icon.png?raw=1")]
    public void ValidateRejectsUntrustedOptionalImageUrls(string imageUrl)
    {
        var app = ValidApp() with { IconUrl = imageUrl };
        Assert.Throws<InvalidDataException>(() => CatalogService.Validate(new AppCatalog { Apps = [app] }));
    }

    [Fact]
    public async Task SaveAndLoadRoundTripPreservesOptionalImageUrls()
    {
        var folder = Path.Combine(Path.GetTempPath(), "moa-catalog-images-" + Guid.NewGuid().ToString("N"));
        try
        {
            var path = Path.Combine(folder, "apps.json");
            var expected = new AppCatalog { Apps = [ValidApp()] };
            await CatalogService.SaveAsync(path, expected);
            var actual = await CatalogService.LoadAsync(path);
            Assert.Equal(expected.Apps[0].IconUrl, actual.Apps[0].IconUrl);
            Assert.Equal(expected.Apps[0].CoverUrl, actual.Apps[0].CoverUrl);
        }
        finally
        {
            if (Directory.Exists(folder)) Directory.Delete(folder, recursive: true);
        }
    }

    [Fact]
    public async Task BundledMoaviewCatalogContainsRepositoryImageUrls()
    {
        var root = new DirectoryInfo(AppContext.BaseDirectory);
        while (root is not null && !File.Exists(Path.Combine(root.FullName, "catalog", "apps.json"))) root = root.Parent;
        Assert.NotNull(root);
        var catalog = await CatalogService.LoadAsync(Path.Combine(root!.FullName, "catalog", "apps.json"));
        var moaview = Assert.Single(catalog.Apps.Where(app => app.Id == "moaview"));
        Assert.Equal("https://raw.githubusercontent.com/hebory/moaview/main/icon.png", moaview.IconUrl);
        Assert.Equal("https://raw.githubusercontent.com/hebory/moaview/main/cover.png", moaview.CoverUrl);
    }

    [Fact]
    public void ValidateAcceptsAnUnpublishedPlaceholderRepository()
    {
        var app = ValidApp() with { Repository = "", IconUrl = null, CoverUrl = null };
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
