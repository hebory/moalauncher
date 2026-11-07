namespace MoaLauncher.Core;

public sealed record AppCatalog
{
    public int SchemaVersion { get; init; } = 1;
    public List<AppDefinition> Apps { get; init; } = [];
}

public sealed record AppDefinition
{
    public string Id { get; init; } = "";
    public string Name { get; init; } = "";
    public string Description { get; init; } = "";
    public string Category { get; init; } = "유틸리티";
    public string AccentColor { get; init; } = "#7C9CFF";
    public string Monogram { get; init; } = "M";
    // Empty until the publisher's repository is known; never guess a source.
    public string Repository { get; init; } = "";
    public List<PackageDefinition> Packages { get; init; } = [];
}

public sealed record PackageDefinition
{
    public string Architecture { get; init; } = "x64";
    public string Kind { get; init; } = "zip";
    public string AssetPattern { get; init; } = "";
    public string ExecutablePath { get; init; } = "";
    // Relative to LocalApplicationData. Required only for Inno Setup packages.
    public string? InstallDirectory { get; init; }
}

public sealed record ReleaseAsset(string Name, Uri DownloadUrl, long Size, string? Sha256);
public sealed record AppRelease(string Tag, string Name, string Notes, Uri PageUrl,
    DateTimeOffset PublishedAt, IReadOnlyList<ReleaseAsset> Assets);
public sealed record ReleaseResult(AppRelease Release, bool IsCached);
public sealed record InstalledApp(string AppId, string Version, string ExecutablePath,
    string InstallDirectory, string PackageKind, DateTimeOffset InstalledAt);
public sealed record InstallProgress(string Stage, long BytesReceived = 0, long? TotalBytes = null)
{
    public double Percent => TotalBytes is > 0 ? Math.Clamp(100d * BytesReceived / TotalBytes.Value, 0, 100) : 0;
}
public sealed record LauncherSettings
{
    public const int DefaultCheckIntervalMinutes = 60;
    public static IReadOnlyList<int> SupportedCheckIntervalsMinutes { get; } =
        Array.AsReadOnly<int>([30, 60, 180, 360, 720, 1440]);

    public bool CheckOnStartup { get; init; } = true;
    public bool PeriodicChecks { get; init; } = true;
    private int checkIntervalMinutes = DefaultCheckIntervalMinutes;
    public int CheckIntervalMinutes
    {
        get => checkIntervalMinutes;
        init => checkIntervalMinutes = SupportedCheckIntervalsMinutes.Contains(value)
            ? value : DefaultCheckIntervalMinutes;
    }
}

public interface IReleaseService
{
    Task<ReleaseResult> GetLatestAsync(AppDefinition app, CancellationToken cancellationToken = default);
}

public interface IInstallationService
{
    Task<InstalledApp?> FindInstalledAsync(AppDefinition app, CancellationToken cancellationToken = default);
    Task<InstalledApp> InstallAsync(AppDefinition app, AppRelease release,
        IProgress<InstallProgress>? progress = null, CancellationToken cancellationToken = default);
    bool IsRunning(InstalledApp app);
    void Launch(InstalledApp app);
}
