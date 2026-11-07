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
    }

    [Fact]
    public void NewSettingsEnableUpdateNotifications()
    {
        Assert.True(new LauncherSettings().UpdateNotifications);
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
