using System.Collections.ObjectModel;
using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Threading;
using MoaLauncher.App;
using MoaLauncher.Core;
using Button = System.Windows.Controls.Button;
using TextBox = System.Windows.Controls.TextBox;

namespace MoaLauncher.App.SmokeTests;

internal sealed class UpdateNotificationChecks
{
    private const BindingFlags PrivateInstance = BindingFlags.Instance | BindingFlags.NonPublic;
    private readonly List<AppDefinition> definitions = [];
    private readonly Dictionary<string, ReleaseResult> releaseResults = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<string, InstalledApp> installed = new(StringComparer.OrdinalIgnoreCase);
    private readonly List<string> installCalls = [];

    internal IReleaseService ReleaseService { get; }
    internal IInstallationService InstallationService { get; }

    internal UpdateNotificationChecks()
    {
        var target = CreateDefinition("smoke-update-target", "테스트 대상 앱", "smoke/target");
        var second = CreateDefinition("smoke-update-second", "두 번째 테스트 앱", "smoke/second");
        var cached = CreateDefinition("smoke-update-cached", "캐시된 테스트 앱", "smoke/cached");
        var uninstalled = CreateDefinition("smoke-update-uninstalled", "미설치 테스트 앱", "smoke/uninstalled");
        var invalidAsset = CreateDefinition("smoke-update-invalid-asset", "자산 오류 테스트 앱", "smoke/invalid-asset");
        var unconnected = CreateDefinition("smoke-update-unconnected", "저장소 미연결 앱", "");
        definitions.AddRange([target, second, cached, uninstalled, invalidAsset, unconnected]);

        installed[target.Id] = MakeInstalled(target, "v1.0.0");
        installed[second.Id] = MakeInstalled(second, "v1.0.0");
        installed[cached.Id] = MakeInstalled(cached, "v1.0.0");
        installed[invalidAsset.Id] = MakeInstalled(invalidAsset, "v1.0.0");

        releaseResults[target.Id] = new ReleaseResult(MakeRelease(target, "v2.0.0"), IsCached: false);
        releaseResults[second.Id] = new ReleaseResult(MakeRelease(second, "v1.5.0"), IsCached: false);
        releaseResults[cached.Id] = new ReleaseResult(MakeRelease(cached, "v2.0.0"), IsCached: true);
        releaseResults[uninstalled.Id] = new ReleaseResult(MakeRelease(uninstalled, "v2.0.0"), IsCached: false);
        releaseResults[invalidAsset.Id] = new ReleaseResult(MakeRelease(invalidAsset, "v2.0.0", includeMatchingAsset: false), IsCached: false);

        ReleaseService = new FakeReleaseService(this);
        InstallationService = new FakeInstallationService(this);
    }

    internal async Task RunAsync(App app, MainWindow window)
    {
        var items = GetField<List<AppItem>>(window, "items");
        items.Clear();
        items.AddRange(definitions.Select(definition => new AppItem(definition)));
        GetMethod(window, "ApplyFilter").Invoke(window, null);

        var tray = GetField<object>(app, "trayIcon");
        var notifyIcon = GetField<System.Windows.Forms.NotifyIcon>(tray, "notifyIcon");
        if (!notifyIcon.Visible) throw new InvalidOperationException("Tray icon was not visible before update notifications.");

        app.HideMainWindow();
        var hiddenTip = notifyIcon.BalloonTipText;
        if (!hiddenTip.Contains("트레이에서 계속 실행", StringComparison.Ordinal))
            throw new InvalidOperationException("The close-to-tray hint was not displayed before update notifications.");

        await CheckAllAsync(window);
        var targetItem = items.Single(item => item.Definition.Id == "smoke-update-target");
        var secondItem = items.Single(item => item.Definition.Id == "smoke-update-second");
        var cachedItem = items.Single(item => item.Definition.Id == "smoke-update-cached");
        var uninstalledItem = items.Single(item => item.Definition.Id == "smoke-update-uninstalled");
        var invalidItem = items.Single(item => item.Definition.Id == "smoke-update-invalid-asset");
        var unconnectedItem = items.Single(item => item.Definition.Id == "smoke-update-unconnected");

        Require(targetItem.HasUpdate && secondItem.HasUpdate, "The eligible installed apps were not marked as updates.");
        Require(cachedItem.HasUpdate && cachedItem.IsCached, "The offline cache fixture did not represent a cached update.");
        Require(uninstalledItem.Installed is null, "The uninstalled fixture unexpectedly acquired an installation.");
        Require(invalidItem.HasUpdate && invalidItem.HasError, "The invalid-asset fixture did not produce an asset validation error.");
        Require(unconnectedItem.NeedsConnection && unconnectedItem.Release is null, "The unconnected fixture unexpectedly checked a release.");
        Require(installCalls.Count == 0, "Checking for updates unexpectedly installed a program.");

        var initialNotice = notifyIcon.BalloonTipText;
        Require(notifyIcon.BalloonTipTitle.Contains("업데이트", StringComparison.Ordinal), "The update notice title was not set.");
        Require(initialNotice.Contains("2개 프로그램 업데이트", StringComparison.Ordinal) &&
                initialNotice.Contains(targetItem.Name, StringComparison.Ordinal) &&
                initialNotice.Contains(secondItem.Name, StringComparison.Ordinal),
            "The initial tray notice did not aggregate the eligible updates.");
        Require(!initialNotice.Contains(cachedItem.Name, StringComparison.Ordinal) &&
                !initialNotice.Contains(uninstalledItem.Name, StringComparison.Ordinal) &&
                !initialNotice.Contains(invalidItem.Name, StringComparison.Ordinal),
            "A cached, uninstalled, or invalid-asset app was included in the update notice.");

        app.HideMainWindow();
        Require(notifyIcon.BalloonTipText == initialNotice,
            "The close-to-tray hint overwrote a previously displayed update notice.");
        const string duplicateSentinel = "중복 알림 검사";
        notifyIcon.BalloonTipText = duplicateSentinel;
        await CheckAllAsync(window);
        Require(notifyIcon.BalloonTipText == duplicateSentinel,
            "A repeated check displayed the same release notification more than once.");

        releaseResults[targetItem.Definition.Id] = new ReleaseResult(MakeRelease(targetItem.Definition, "v2.1.0"), IsCached: false);
        await CheckAllAsync(window);
        var newVersionNotice = notifyIcon.BalloonTipText;
        Require(newVersionNotice != initialNotice && newVersionNotice.Contains(targetItem.Name, StringComparison.Ordinal),
            "A newly published release did not produce a fresh tray notice.");

        GetField<TextBox>(window, "SearchBox").Text = "검색 결과가 없는 단어";
        GetField<Button>(window, "SettingsNav").RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
        RaiseBalloonTipClicked(notifyIcon);
        await WaitUntilAsync(() => window.IsVisible && GetField<string>(window, "filter") == "updates", window.Dispatcher,
            "notification click opening the update list");
        await window.Dispatcher.InvokeAsync(() =>
        {
            window.UpdateLayout();
            GetField<ItemsControl>(window, "AppCards").UpdateLayout();
        }, DispatcherPriority.ApplicationIdle);

        Require(GetField<TextBox>(window, "SearchBox").Text.Length == 0,
            "The notification navigation did not clear the search filter.");
        Require(GetField<FrameworkElement>(window, "LibraryView").Visibility == Visibility.Visible &&
                GetField<FrameworkElement>(window, "SettingsView").Visibility == Visibility.Collapsed,
            "The notification navigation did not leave the settings view.");
        var visible = GetField<ObservableCollection<AppItem>>(window, "visibleItems");
        Require(visible.Take(2).Select(item => item.Definition.Id).SequenceEqual([targetItem.Definition.Id, secondItem.Definition.Id]),
            "The notification navigation did not put the notified app first in the update list.");
        Require(GetField<TextBlock>(window, "StatusText").Text.Contains("업데이트", StringComparison.Ordinal),
            "The update list did not explain how to install the notified update.");

        var updateButton = FindActionButton(window, targetItem.Definition.Id)
            ?? throw new InvalidOperationException("Could not find the notified app's update button.");
        Require(updateButton.Content as string == "업데이트" && updateButton.IsEnabled,
            "The notified app did not expose an enabled update action.");
        Require(installCalls.Count == 0, "The notification click installed an app before the user selected its update button.");
        releaseResults[targetItem.Definition.Id] = new ReleaseResult(MakeRelease(targetItem.Definition, "v2.2.0"), IsCached: false);
        updateButton.RaiseEvent(new RoutedEventArgs(Button.ClickEvent, updateButton));
        await WaitUntilAsync(() => installCalls.Count == 1 && !targetItem.Busy && targetItem.Installed?.Version == "v2.2.0",
            window.Dispatcher, "the selected app update completing");

        Require(installCalls.SequenceEqual([targetItem.Definition.Id], StringComparer.OrdinalIgnoreCase),
            "The update action installed an app other than the selected target.");
        Require(!targetItem.HasUpdate && secondItem.HasUpdate,
            "The update list did not reflect completion for the selected app only.");
        Require(!visible.Contains(targetItem) && visible[0].Definition.Id == secondItem.Definition.Id,
            "The completed app remained in the update list or another app disappeared.");

        secondItem.Installed = MakeInstalled(secondItem.Definition, secondItem.Release!.Tag);
        app.HideMainWindow();
        RaiseBalloonTipClicked(notifyIcon);
        await WaitUntilAsync(() => window.IsVisible, window.Dispatcher, "late notification restoring the current list");
        Require(!visible.Contains(targetItem) && !visible.Contains(secondItem) && installCalls.Count == 1,
            "A late notification restored completed updates or started another install.");

        Console.WriteLine("PASS: aggregated update notifications, stale/error exclusions, deduplication, tray click navigation, and selected-app update.");
    }

    private static async Task CheckAllAsync(MainWindow window)
    {
        var check = GetMethod(window, "CheckAllAsync").Invoke(window, null) as Task
            ?? throw new InvalidOperationException("CheckAllAsync did not return a Task.");
        await check;
    }

    private static void RaiseBalloonTipClicked(System.Windows.Forms.NotifyIcon notifyIcon)
    {
        var handler = typeof(System.Windows.Forms.NotifyIcon).GetMethod("OnBalloonTipClicked", PrivateInstance)
            ?? throw new MissingMethodException(typeof(System.Windows.Forms.NotifyIcon).FullName, "OnBalloonTipClicked");
        handler.Invoke(notifyIcon, null);
    }

    private static Button? FindActionButton(DependencyObject root, string appId)
    {
        var count = VisualTreeHelper.GetChildrenCount(root);
        for (var index = 0; index < count; index++)
        {
            var child = VisualTreeHelper.GetChild(root, index);
            if (child is Button button && button.DataContext is AppItem item && item.Definition.Id == appId)
                return button;
            var nested = FindActionButton(child, appId);
            if (nested is not null) return nested;
        }
        return null;
    }

    private static AppDefinition CreateDefinition(string id, string name, string repository)
    {
        var token = id.Replace("smoke-update-", "", StringComparison.OrdinalIgnoreCase);
        return new AppDefinition
        {
            Id = id,
            Name = name,
            Description = "스모크 테스트용 업데이트 항목",
            Category = "테스트",
            AccentColor = "#8BA3FF",
            Monogram = "T",
            Repository = repository,
            Packages = new[] { "x64", "x86", "arm64" }.Select(architecture => new PackageDefinition
            {
                Architecture = architecture,
                Kind = "zip",
                AssetPattern = $"Moa-{token}-*-{architecture}.zip",
                ExecutablePath = "Moa.exe"
            }).ToList()
        };
    }

    private static AppRelease MakeRelease(AppDefinition definition, string tag, bool includeMatchingAsset = true)
    {
        var assets = new List<ReleaseAsset>();
        foreach (var package in definition.Packages)
        {
            var name = includeMatchingAsset
                ? $"Moa-{definition.Id.Replace("smoke-update-", "", StringComparison.OrdinalIgnoreCase)}-{tag}-{package.Architecture}.zip"
                : $"Other-{tag}-{package.Architecture}.zip";
            assets.Add(new ReleaseAsset(name,
                new Uri($"https://github.com/{definition.Repository}/releases/download/{tag}/{name}"),
                100, null));
        }
        return new AppRelease(tag, $"{definition.Name} {tag}", "스모크 릴리스", new Uri($"https://github.com/{definition.Repository}/releases/tag/{tag}"),
            DateTimeOffset.UtcNow, assets);
    }

    private static InstalledApp MakeInstalled(AppDefinition definition, string version) =>
        new(definition.Id, version, Path.Combine(Path.GetTempPath(), "MoaSmoke", definition.Id, "Moa.exe"),
            Path.Combine(Path.GetTempPath(), "MoaSmoke", definition.Id), "zip", DateTimeOffset.UtcNow);

    private static async Task WaitUntilAsync(Func<bool> condition, Dispatcher dispatcher, string description)
    {
        var watch = Stopwatch.StartNew();
        while (!condition())
        {
            if (watch.Elapsed > TimeSpan.FromSeconds(10)) throw new TimeoutException($"Timed out waiting for {description}.");
            await Task.Delay(20);
            dispatcher.VerifyAccess();
        }
    }

    private static T GetField<T>(object instance, string name)
    {
        var field = instance.GetType().GetField(name, PrivateInstance)
            ?? throw new MissingFieldException(instance.GetType().FullName, name);
        return field.GetValue(instance) is T value
            ? value
            : throw new InvalidCastException($"Field {name} is not {typeof(T).Name}.");
    }

    private static MethodInfo GetMethod(object instance, string name) =>
        instance.GetType().GetMethod(name, PrivateInstance)
        ?? throw new MissingMethodException(instance.GetType().FullName, name);

    private static void Require(bool condition, string message)
    {
        if (!condition) throw new InvalidOperationException(message);
    }

    private sealed class FakeReleaseService(UpdateNotificationChecks owner) : IReleaseService
    {
        public Task<ReleaseResult> GetLatestAsync(AppDefinition app, CancellationToken cancellationToken = default)
        {
            cancellationToken.ThrowIfCancellationRequested();
            return owner.releaseResults.TryGetValue(app.Id, out var result)
                ? Task.FromResult(result)
                : Task.FromException<ReleaseResult>(new InvalidOperationException($"No smoke release configured for {app.Id}."));
        }
    }

    private sealed class FakeInstallationService(UpdateNotificationChecks owner) : IInstallationService
    {
        public Task<InstalledApp?> FindInstalledAsync(AppDefinition app, CancellationToken cancellationToken = default)
        {
            cancellationToken.ThrowIfCancellationRequested();
            owner.installed.TryGetValue(app.Id, out var appInstalled);
            return Task.FromResult(appInstalled);
        }

        public Task<InstalledApp> InstallAsync(AppDefinition app, AppRelease release,
            IProgress<InstallProgress>? progress = null, CancellationToken cancellationToken = default)
        {
            cancellationToken.ThrowIfCancellationRequested();
            owner.installCalls.Add(app.Id);
            var result = MakeInstalled(app, release.Tag);
            owner.installed[app.Id] = result;
            progress?.Report(new InstallProgress("설치 완료", 100, 100));
            return Task.FromResult(result);
        }

        public bool IsRunning(InstalledApp app) => false;
        public void Launch(InstalledApp app) { }
    }
}
