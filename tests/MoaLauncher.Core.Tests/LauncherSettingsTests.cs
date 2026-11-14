using System.Text.Json;
using MoaLauncher.Core;

namespace MoaLauncher.Core.Tests;

public sealed class LauncherSettingsTests
{
    [Fact]
    public void LegacySettingsUseOneHourAndPreserveExistingPreferences()
    {
        var settings = JsonSerializer.Deserialize<LauncherSettings>(
            """{"checkOnStartup":false,"periodicChecks":false}""", CatalogService.JsonOptions)!;

        Assert.Equal(60, settings.CheckIntervalMinutes);
        Assert.False(settings.CheckOnStartup);
        Assert.False(settings.PeriodicChecks);
        Assert.True(settings.UpdateNotifications);
        Assert.False(settings.UseGridView);
    }

    [Fact]
    public void NewSettingsEnableUpdateNotifications()
    {
        Assert.True(new LauncherSettings().UpdateNotifications);
    }

    [Fact]
    public void NewSettingsKeepTheListViewByDefault()
    {
        Assert.False(new LauncherSettings().UseGridView);
    }

    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public void SavedLibraryViewSurvivesRestartWithoutResettingOtherPreferences(bool useGridView)
    {
        var original = new LauncherSettings
        {
            UseGridView = useGridView,
            CheckOnStartup = false,
            PeriodicChecks = false,
            UpdateNotifications = false,
            CheckIntervalMinutes = 1440
        };

        var json = JsonSerializer.Serialize(original, CatalogService.JsonOptions);
        var restored = JsonSerializer.Deserialize<LauncherSettings>(json, CatalogService.JsonOptions)!;

        Assert.Equal(useGridView, restored.UseGridView);
        Assert.False(restored.CheckOnStartup);
        Assert.False(restored.PeriodicChecks);
        Assert.False(restored.UpdateNotifications);
        Assert.Equal(1440, restored.CheckIntervalMinutes);
    }

    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public void SavedNotificationPreferenceSurvivesRestart(bool enabled)
    {
        var original = new LauncherSettings { UpdateNotifications = enabled, CheckIntervalMinutes = 1440 };
        var json = JsonSerializer.Serialize(original, CatalogService.JsonOptions);
        var restored = JsonSerializer.Deserialize<LauncherSettings>(json, CatalogService.JsonOptions)!;

        Assert.Equal(enabled, restored.UpdateNotifications);
        Assert.Equal(1440, restored.CheckIntervalMinutes);
        Assert.True(restored.CheckOnStartup);
        Assert.True(restored.PeriodicChecks);
    }

    [Theory]
    [InlineData(30)]
    [InlineData(60)]
    [InlineData(180)]
    [InlineData(360)]
    [InlineData(720)]
    [InlineData(1440)]
    public void SavedIntervalSurvivesRestart(int minutes)
    {
        var original = new LauncherSettings { CheckIntervalMinutes = minutes, PeriodicChecks = false };
        var json = JsonSerializer.Serialize(original, CatalogService.JsonOptions);
        var restored = JsonSerializer.Deserialize<LauncherSettings>(json, CatalogService.JsonOptions)!;

        Assert.Equal(minutes, restored.CheckIntervalMinutes);
        Assert.False(restored.PeriodicChecks);
    }

    [Theory]
    [InlineData(0)]
    [InlineData(-1)]
    [InlineData(90)]
    [InlineData(int.MaxValue)]
    public void InvalidSavedIntervalFallsBackToOneHour(int minutes)
    {
        var json = $$"""{"checkIntervalMinutes":{{minutes}}}""";
        var settings = JsonSerializer.Deserialize<LauncherSettings>(json, CatalogService.JsonOptions)!;

        Assert.Equal(60, settings.CheckIntervalMinutes);
    }
}
