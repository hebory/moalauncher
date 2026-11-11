using System.Collections.ObjectModel;
using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Threading;
using MoaLauncher.App;
using MoaLauncher.Core;
using Button = System.Windows.Controls.Button;
using TextBox = System.Windows.Controls.TextBox;

namespace MoaLauncher.App.SmokeTests;

internal sealed partial class UpdateNotificationChecks
{
    private const BindingFlags PrivateInstance = BindingFlags.Instance | BindingFlags.NonPublic;
    private readonly List<AppDefinition> definitions = [];
    private readonly Dictionary<string, ReleaseResult> releaseResults = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<string, InstalledApp> installed = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<string, int> releaseCalls = new(StringComparer.OrdinalIgnoreCase);
    private readonly List<string> releaseCallOrder = [];
    private readonly List<string> installCalls = [];
    private readonly HashSet<string> failedInstallIds = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<string, InstalledApp> refreshedInstallations = new(StringComparer.OrdinalIgnoreCase);
    private TaskCompletionSource? installStarted;
    private TaskCompletionSource? installGate;
    private int installDelayMilliseconds;
    private bool installReportsInstallerStarted;
    private int activeInstallCount;
    private int maximumConcurrentInstalls;

    internal IReleaseService ReleaseService { get; }
    internal IInstallationService InstallationService { get; }

    internal UpdateNotificationChecks()
    {
        var launcher = SelfUpdateService.Definition;
        var launcherVersion = ReadLauncherVersion();
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
        installed[launcher.Id] = MakeInstalled(launcher, launcherVersion);

        releaseResults[target.Id] = new ReleaseResult(MakeRelease(target, "v2.0.0"), IsCached: false);
        releaseResults[second.Id] = new ReleaseResult(MakeRelease(second, "v1.5.0"), IsCached: false);
        releaseResults[cached.Id] = new ReleaseResult(MakeRelease(cached, "v2.0.0"), IsCached: true);
        releaseResults[uninstalled.Id] = new ReleaseResult(MakeRelease(uninstalled, "v2.0.0"), IsCached: false);
        releaseResults[invalidAsset.Id] = new ReleaseResult(MakeRelease(invalidAsset, "v2.0.0", includeMatchingAsset: false), IsCached: false);
        releaseResults[launcher.Id] = new ReleaseResult(MakeLauncherRelease(launcher, ToReleaseTag(launcherVersion)), IsCached: false);

        ReleaseService = new FakeReleaseService(this);
        InstallationService = new FakeInstallationService(this);
    }

    internal async Task RunAsync(App app, MainWindow window)
    {
        var items = GetField<List<AppItem>>(window, "items");
        var launcherItem = items.SingleOrDefault(item => item.Definition.Id == SelfUpdateService.Definition.Id)
            ?? throw new InvalidOperationException("The built-in Moa Launcher item was not added to the library.");
        Require(GetProperty<bool>(launcherItem, "IsLauncher"), "The built-in launcher was not marked as a self-update item.");
        Require(launcherItem.Installed is not null, "The built-in launcher did not detect its current installation.");
        Require(!GetProperty<bool>(launcherItem, "CanEdit") && !GetProperty<bool>(launcherItem, "CanLaunch"),
            "The built-in launcher still exposes app editing or launch actions.");
        Require(launcherItem.Definition.IconUrl == "https://raw.githubusercontent.com/hebory/moalauncher/master/icon.png" &&
                launcherItem.Definition.CoverUrl == "https://raw.githubusercontent.com/hebory/moalauncher/master/cover.png",
            "The built-in launcher artwork URLs do not point to the public master-branch PNG assets.");
        Require(launcherItem.IconImage is { IsFrozen: true, PixelWidth: 512, PixelHeight: 512 } &&
                launcherItem.CoverImage is { IsFrozen: true, PixelWidth: 1672, PixelHeight: 941 },
            "The built-in launcher did not load its frozen packaged icon and cover at startup.");
        var launcherVersion = launcherItem.Installed!.Version;
        releaseResults[launcherItem.Definition.Id] = new ReleaseResult(
            MakeLauncherRelease(launcherItem.Definition, ToReleaseTag(launcherVersion)), IsCached: false);
        var originalVisible = GetField<ObservableCollection<AppItem>>(window, "visibleItems");
        Require(originalVisible.Contains(launcherItem) && originalVisible.Count == items.Count,
            "The built-in launcher was missing from the initial program list.");
        Require(GetField<TextBlock>(window, "LibrarySummary").Text == $"{originalVisible.Count}개 프로그램",
            "The initial program count did not match the visible library including the launcher.");

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

        var notificationCheck = GetField<System.Windows.Controls.CheckBox>(window, "UpdateNotificationCheck");
        Require(notificationCheck.IsEnabled && notificationCheck.IsChecked == true,
            "Legacy settings without the notification preference did not enable the notification toggle.");
        await SetNotificationsAsync(window, false);
        // Other settings share the save handler and must not reset the notification preference.
        var periodicCheck = GetField<System.Windows.Controls.CheckBox>(window, "PeriodicCheck");
        periodicCheck.IsChecked = false;
        await AssertSavedPreferenceAsync(window, false);
        periodicCheck.IsChecked = true;
        await AssertSavedPreferenceAsync(window, false);
        var timer = GetField<DispatcherTimer>(window, "timer");
        Require(timer.IsEnabled && timer.Interval == TimeSpan.FromHours(1),
            "Disabling notifications changed the periodic update schedule.");

        const string disabledSentinel = "알림 꺼짐 검사";
        notifyIcon.BalloonTipText = disabledSentinel;
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
        Require(notifyIcon.BalloonTipText == disabledSentinel,
            "An update notification was sent while the preference was disabled.");

        await SetNotificationsAsync(window, true);
        await CheckAllAsync(window);

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
        await SetNotificationsAsync(window, false);
        await SetNotificationsAsync(window, true);
        await CheckAllAsync(window);
        Require(notifyIcon.BalloonTipText == duplicateSentinel,
            "Toggling notifications discarded the already-announced release history.");

        await SetNotificationsAsync(window, false);
        releaseResults[targetItem.Definition.Id] = new ReleaseResult(MakeRelease(targetItem.Definition, "v2.1.0"), IsCached: false);
        await CheckAllAsync(window);
        Require(targetItem.Release?.Tag == "v2.1.0" && notifyIcon.BalloonTipText == duplicateSentinel,
            "A muted new version was not checked silently.");
        await SetNotificationsAsync(window, true);
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
        installed[secondItem.Definition.Id] = secondItem.Installed;
        app.HideMainWindow();
        RaiseBalloonTipClicked(notifyIcon);
        await WaitUntilAsync(() => window.IsVisible, window.Dispatcher, "late notification restoring the current list");
        Require(!visible.Contains(targetItem) && !visible.Contains(secondItem) && installCalls.Count == 1,
            "A late notification restored completed updates or started another install.");

        await CheckLauncherSelfUpdateAsync(app, window, items, visible, launcherItem, launcherVersion, notifyIcon);

        await RunBulkUpdateChecksAsync(window, items, launcherItem);

        Console.WriteLine("PASS: notification settings, app updates, built-in launcher notice navigation, and sequential bulk updates.");
    }

    private async Task CheckLauncherSelfUpdateAsync(App app, MainWindow window, List<AppItem> items,
        ObservableCollection<AppItem> visible, AppItem launcherItem, string currentVersion,
        System.Windows.Forms.NotifyIcon notifyIcon)
    {
        items.Clear();
        var secondItem = new AppItem(definitions.Single(definition => definition.Id == "smoke-update-second"));
        items.Add(launcherItem);
        items.Add(secondItem);
        installed[secondItem.Definition.Id] = MakeInstalled(secondItem.Definition, "v1.5.0");
        releaseResults[secondItem.Definition.Id] = new ReleaseResult(MakeRelease(secondItem.Definition, "v1.5.0"), IsCached: false);
        GetMethod(window, "ApplyFilter").Invoke(window, null);
        releaseResults[launcherItem.Definition.Id] = new ReleaseResult(
            MakeLauncherRelease(launcherItem.Definition, "v99.0.0"), IsCached: false);

        await CheckAllAsync(window);
        Require(launcherItem.Installed?.Version == currentVersion,
            "The self-update check changed the detected installed launcher version.");
        Require(launcherItem.HasUpdate && launcherItem.ActionText == "업데이트 후 재시작",
            "A newer launcher release did not expose the self-update action.");
        Require(!GetProperty<bool>(launcherItem, "CanEdit") && !GetProperty<bool>(launcherItem, "CanLaunch"),
            "The self-update item exposed app editing or normal launch while an update was available.");
        Require(installCalls.Count == 1, "Checking for the launcher update unexpectedly installed a program.");
        Require(notifyIcon.BalloonTipText.Contains(launcherItem.Name, StringComparison.Ordinal),
            "The launcher update was not included in the tray notification.");

        InvokeClickHandler(window, "All_Click");
        Require(visible.SequenceEqual([launcherItem, secondItem]),
            "The launcher was not shown alongside other programs while its update was pending.");
        Require(GetField<TextBlock>(window, "LibrarySummary").Text == "2개 프로그램",
            "The normal program count did not include the launcher update item.");
        Require(GetField<Button>(window, "UpdatesNav").Content as string == "업데이트 1",
            "The update badge did not include the launcher update.");
        Require(GetField<TextBlock>(window, "StatusText").Text == "확인 완료 · 업데이트 가능한 프로그램 1개",
            "The check summary did not include the launcher update.");

        GetField<TextBox>(window, "SearchBox").Text = launcherItem.Repository;
        Require(visible.Count == 1 && visible[0] == launcherItem,
            "Searching for the launcher repository did not show its program card.");
        GetField<TextBox>(window, "SearchBox").Clear();

        InvokeClickHandler(window, "Installed_Click");
        Require(visible.SequenceEqual([launcherItem, secondItem]),
            "The launcher was missing from the installed-app list.");
        Require(GetField<TextBlock>(window, "LibrarySummary").Text == "2개 프로그램",
            "The installed-app count did not include the launcher.");

        InvokeClickHandler(window, "Updates_Click");
        Require(visible.Count == 1 && visible[0] == launcherItem,
            "The update list did not show the pending launcher update on its own.");
        Require(GetField<TextBlock>(window, "LibrarySummary").Text == "1개 프로그램",
            "The updates count did not include the launcher update.");

        GetField<TextBox>(window, "SearchBox").Text = "런처 알림 테스트 검색어";
        GetField<Button>(window, "SettingsNav").RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
        app.HideMainWindow();
        RaiseBalloonTipClicked(notifyIcon);
        await WaitUntilAsync(() => window.IsVisible &&
            GetField<FrameworkElement>(window, "LibraryView").Visibility == Visibility.Visible &&
            GetField<string>(window, "filter") == "updates" && visible.Contains(launcherItem),
            window.Dispatcher, "self-update notification opening the launcher program card");
        await window.Dispatcher.InvokeAsync(() =>
        {
            window.UpdateLayout();
        }, DispatcherPriority.ApplicationIdle);

        var launcherAction = FindActionButton(GetField<ItemsControl>(window, "AppCards"), launcherItem.Definition.Id)
            ?? throw new InvalidOperationException("Could not find the launcher card's update button.");
        Require(launcherAction.Content as string == "업데이트 후 재시작" && launcherAction.IsEnabled,
            "The launcher program card did not expose its enabled update action.");
        Require(GetField<TextBox>(window, "SearchBox").Text.Length == 0 &&
                GetField<FrameworkElement>(window, "SettingsView").Visibility == Visibility.Collapsed,
            "The launcher notification did not clear search and leave Settings.");
        Require(GetField<TextBlock>(window, "StatusText").Text.Contains("업데이트 후 재시작", StringComparison.Ordinal),
            "The launcher notification did not explain the displayed self-update action.");
        Require(installCalls.Count == 1,
            "Opening the self-update notification executed the installer; the self-update button must remain untouched in this smoke test.");

        var refreshButton = GetField<Button>(window, "RefreshButton");
        Require(refreshButton.IsEnabled, "The shared version-check button was disabled.");
        var launcherChecksBefore = GetReleaseCallCount(launcherItem.Definition.Id);
        var appChecksBefore = GetReleaseCallCount(secondItem.Definition.Id);
        refreshButton.RaiseEvent(new RoutedEventArgs(Button.ClickEvent, refreshButton));
        await WaitUntilAsync(() => GetReleaseCallCount(launcherItem.Definition.Id) > launcherChecksBefore &&
            GetReleaseCallCount(secondItem.Definition.Id) > appChecksBefore && !GetField<bool>(window, "checking"),
            window.Dispatcher, "shared launcher and app version check");

        // Announcing an ordinary update together with a launcher update should open the regular update list.
        installed[secondItem.Definition.Id] = MakeInstalled(secondItem.Definition, "v1.0.0");
        secondItem.Installed = installed[secondItem.Definition.Id];
        releaseResults[secondItem.Definition.Id] = new ReleaseResult(MakeRelease(secondItem.Definition, "v1.6.0"), IsCached: false);
        releaseResults[launcherItem.Definition.Id] = new ReleaseResult(
            MakeLauncherRelease(launcherItem.Definition, "v100.0.0"), IsCached: false);
        await CheckAllAsync(window);
        var mixedNotice = notifyIcon.BalloonTipText;
        Require(mixedNotice.Contains("2개 프로그램 업데이트", StringComparison.Ordinal) &&
                mixedNotice.Contains(secondItem.Name, StringComparison.Ordinal) &&
                mixedNotice.Contains(launcherItem.Name, StringComparison.Ordinal),
            "A mixed app and launcher update notification was not aggregated.");
        Require(GetField<Button>(window, "UpdatesNav").Content as string == "업데이트 2",
            "The update badge did not count both the launcher and app update.");

        GetField<TextBox>(window, "SearchBox").Text = "혼합 알림 테스트 검색어";
        app.HideMainWindow();
        RaiseBalloonTipClicked(notifyIcon);
        await WaitUntilAsync(() => window.IsVisible && GetField<string>(window, "filter") == "updates" && visible.Contains(secondItem),
            window.Dispatcher, "mixed notification opening the regular app update list");
        Require(visible.Count == 2 && visible.Contains(launcherItem) && visible.Contains(secondItem),
            "The mixed notification did not show the launcher and app in the same update list.");
        Require(GetField<TextBox>(window, "SearchBox").Text.Length == 0,
            "The mixed notification did not clear the regular app search filter.");
        Require(GetField<TextBlock>(window, "StatusText").Text.Contains("2개 프로그램", StringComparison.Ordinal),
            "The mixed notification did not explain both updates in the program list.");

        // The notification can be clicked after both releases are already installed.
        installed[secondItem.Definition.Id] = MakeInstalled(secondItem.Definition, "v1.6.0");
        installed[launcherItem.Definition.Id] = MakeInstalled(launcherItem.Definition, "v100.0.0");
        launcherItem.Installed = installed[launcherItem.Definition.Id];
        await CheckAllAsync(window);
        Require(!secondItem.HasUpdate && !launcherItem.HasUpdate,
            "The late-notification fixture still has an available update.");
        app.HideMainWindow();
        RaiseBalloonTipClicked(notifyIcon);
        await WaitUntilAsync(() => window.IsVisible &&
            GetField<FrameworkElement>(window, "LibraryView").Visibility == Visibility.Visible &&
            GetField<string>(window, "filter") == "updates",
            window.Dispatcher, "late launcher notification opening the current update list");
        Require(visible.Count == 0 && GetField<FrameworkElement>(window, "EmptyPanel").Visibility == Visibility.Visible &&
                GetField<Button>(window, "UpdatesNav").Content as string == "업데이트",
            "A late launcher notification restored a completed update or left an incorrect count.");
        InvokeClickHandler(window, "All_Click");
        await window.Dispatcher.InvokeAsync(window.UpdateLayout, DispatcherPriority.ApplicationIdle);
        var completedAction = FindActionButton(GetField<ItemsControl>(window, "AppCards"), launcherItem.Definition.Id)
            ?? throw new InvalidOperationException("The completed launcher card disappeared from the library.");
        Require(completedAction.Content as string == "최신 버전" && !completedAction.IsEnabled,
            "The completed launcher card did not show its disabled latest-version action.");
        Require(installCalls.Count == 1, "The launcher update action was executed during notification navigation.");
    }

    private static void InvokeClickHandler(MainWindow window, string methodName) =>
        GetMethod(window, methodName).Invoke(window, [window, new RoutedEventArgs()]);

    private static async Task SetNotificationsAsync(MainWindow window, bool enabled)
    {
        GetField<System.Windows.Controls.CheckBox>(window, "UpdateNotificationCheck").IsChecked = enabled;
        await AssertSavedPreferenceAsync(window, enabled);
    }

    private static async Task AssertSavedPreferenceAsync(MainWindow window, bool enabled)
    {
        var gate = GetField<SemaphoreSlim>(window, "settingsGate");
        await gate.WaitAsync();
        try
        {
            var saved = JsonSerializer.Deserialize<LauncherSettings>(
                await File.ReadAllTextAsync(Path.Combine(App.DataDirectory, "settings.json")), CatalogService.JsonOptions)!;
            Require(saved.UpdateNotifications == enabled && GetField<LauncherSettings>(window, "settings").UpdateNotifications == enabled,
                "The notification preference was not applied and saved consistently.");
        }
        finally { gate.Release(); }
    }

    private static async Task CheckAllAsync(MainWindow window)
    {
        var check = GetMethod(window, "CheckAllAsync").Invoke(window, null) as Task
            ?? throw new InvalidOperationException("CheckAllAsync did not return a Task.");
        await check;
    }

    private int GetReleaseCallCount(string appId) => releaseCalls.TryGetValue(appId, out var count) ? count : 0;

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

    private static AppRelease MakeLauncherRelease(AppDefinition definition, string tag)
    {
        Require(definition.Packages.Any(package => package.Architecture == "x64" && package.AssetPattern == SelfUpdateService.InstallerPattern),
            "The built-in launcher package does not match the self-update installer pattern.");
        var name = $"MoaLauncher-{tag}-Setup-x64.exe";
        var asset = new ReleaseAsset(name,
            new Uri($"https://github.com/{SelfUpdateService.Repository}/releases/download/{tag}/{name}"), 1024, null);
        return new AppRelease(tag, $"Moa Launcher {tag}", "스모크 테스트 런처 릴리스",
            new Uri($"https://github.com/{SelfUpdateService.Repository}/releases/tag/{tag}"), DateTimeOffset.UtcNow, [asset]);
    }

    private static string ToReleaseTag(string version) => version.StartsWith("v", StringComparison.OrdinalIgnoreCase)
        ? version : "v" + version;

    private static string ReadLauncherVersion()
    {
        var method = typeof(App).GetMethod("GetVersion", BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic,
            binder: null, types: Type.EmptyTypes, modifiers: null);
        if (method?.Invoke(null, null) is string methodVersion && !string.IsNullOrWhiteSpace(methodVersion)) return methodVersion;

        var property = typeof(App).GetProperty("CurrentVersion", BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic);
        if (property?.GetValue(null) is string propertyVersion && !string.IsNullOrWhiteSpace(propertyVersion)) return propertyVersion;

        var productVersion = System.Diagnostics.FileVersionInfo.GetVersionInfo(typeof(MainWindow).Assembly.Location).ProductVersion;
        if (!string.IsNullOrWhiteSpace(productVersion)) return productVersion.Split('+')[0];
        var assemblyVersion = typeof(MainWindow).Assembly.GetName().Version
            ?? throw new InvalidOperationException("Could not determine the Moa Launcher version for its self-update smoke test.");
        return $"{assemblyVersion.Major}.{assemblyVersion.Minor}.{Math.Max(assemblyVersion.Build, 0)}";
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

    private static T GetProperty<T>(object instance, string name)
    {
        var property = instance.GetType().GetProperty(name, BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic)
            ?? throw new MissingMemberException(instance.GetType().FullName, name);
        return property.GetValue(instance) is T value
            ? value
            : throw new InvalidCastException($"Property {name} is not {typeof(T).Name}.");
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
            owner.releaseCalls[app.Id] = owner.GetReleaseCallCount(app.Id) + 1;
            owner.releaseCallOrder.Add(app.Id);
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
            if (owner.refreshedInstallations.Remove(app.Id, out var refreshed))
                owner.installed[app.Id] = refreshed;
            owner.installed.TryGetValue(app.Id, out var appInstalled);
            return Task.FromResult(appInstalled);
        }

        public async Task<InstalledApp> InstallAsync(AppDefinition app, AppRelease release,
            IProgress<InstallProgress>? progress = null, CancellationToken cancellationToken = default)
        {
            cancellationToken.ThrowIfCancellationRequested();
            owner.installCalls.Add(app.Id);
            owner.installStarted?.TrySetResult();
            var active = Interlocked.Increment(ref owner.activeInstallCount);
            owner.maximumConcurrentInstalls = Math.Max(owner.maximumConcurrentInstalls, active);
            try
            {
                if (owner.installReportsInstallerStarted)
                    progress?.Report(new InstallProgress("설치 프로그램 실행", 20, 100));
                if (owner.installGate is { } gate)
                    await gate.Task.WaitAsync(cancellationToken);
                else if (owner.installDelayMilliseconds > 0)
                    await Task.Delay(owner.installDelayMilliseconds, cancellationToken);
                cancellationToken.ThrowIfCancellationRequested();
                if (owner.failedInstallIds.Contains(app.Id)) throw new IOException($"Fake installation failed for {app.Id}.");
                var result = MakeInstalled(app, release.Tag);
                owner.installed[app.Id] = result;
                progress?.Report(new InstallProgress("설치 완료", 100, 100));
                return result;
            }
            finally { Interlocked.Decrement(ref owner.activeInstallCount); }
        }

        public bool IsRunning(InstalledApp app) => false;
        public void Launch(InstalledApp app) { }
    }
}
