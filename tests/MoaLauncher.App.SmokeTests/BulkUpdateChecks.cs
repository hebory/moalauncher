using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Reflection;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Threading;
using MoaLauncher.App;
using MoaLauncher.Core;
using Button = System.Windows.Controls.Button;
using Point = System.Windows.Point;
using TextBox = System.Windows.Controls.TextBox;

namespace MoaLauncher.App.SmokeTests;

internal sealed partial class UpdateNotificationChecks
{
    private async Task RunBulkUpdateChecksAsync(App app, MainWindow window, List<AppItem> items, AppItem launcherItem)
    {
        await CheckSuccessfulBulkUpdateAsync(app, window, items, launcherItem);
        await CheckBulkFailureStopsRemainderAsync(window, items, launcherItem);
        await CheckBulkCancellationStopsRemainderAsync(window, items, launcherItem);
        await CheckBatchStopWaitsForInstallerAsync(window, items, launcherItem);
        await CheckNoUpdatesDisablesBulkButtonsAsync(app, window, items, launcherItem);
        await CheckMinimumWidthHeaderLayoutAsync(window);
    }

    private async Task CheckSuccessfulBulkUpdateAsync(App app, MainWindow window, List<AppItem> items, AppItem launcherItem)
    {
        var launcherVersionBeforeBatch = launcherItem.Installed?.Version
            ?? throw new InvalidOperationException("The launcher had no installed version before the batch fixture.");
        var target = SetupBulkFixture(window, items, launcherItem, launcherHasUpdate: true);
        var targetItem = target.Single(item => item.Definition.Id == "smoke-update-target");
        var secondItem = target.Single(item => item.Definition.Id == "smoke-update-second");
        var thirdItem = target.Single(item => item.Definition.Id == "smoke-update-cached");

        // The filter/search are deliberately empty while the batch must still use all known updates.
        SetField(window, "filter", "updates");
        GetField<TextBox>(window, "SearchBox").Text = "검색과 무관한 항목";
        InvokeNoArg(window, "ApplyFilter");
        Require(GetField<ObservableCollection<AppItem>>(window, "visibleItems").Count == 0,
            "The bulk fixture unexpectedly remained visible through the search filter.");

        // target has an update in the list snapshot, but the fresh installed-state lookup finds it already current.
        refreshedInstallations[targetItem.Definition.Id] = MakeInstalled(targetItem.Definition, "v2.0.0");
        releaseResults[secondItem.Definition.Id] = new ReleaseResult(MakeRelease(secondItem.Definition, "v2.1.0"), IsCached: false);
        installDelayMilliseconds = 0;
        installStarted = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        installGate = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var updateAll = GetField<Button>(window, "UpdateAllButton");
        var stop = GetField<Button>(window, "StopUpdatesButton");
        InvokeNoArg(window, "RefreshActionAvailability");
        Require(updateAll.IsEnabled, "The update-all button was disabled despite pending updates.");
        Require(!stop.IsEnabled && stop.Visibility == Visibility.Collapsed,
            "The stop button was available before a bulk update started.");

        var tray = GetField<object>(app, "trayIcon");
        var menu = GetField<System.Windows.Forms.ContextMenuStrip>(tray, "contextMenu");
        var trayUpdateAll = FindTrayUpdateAllItem(menu);
        RefreshTrayMenu(menu);
        Require(trayUpdateAll.Enabled, "The tray update-all action was disabled despite pending updates.");

        var editor = new AppEditorWindow(targetItem.Definition) { Owner = window };
        editor.Show();
        try
        {
            GetField<TextBox>(editor, "NameInput").Text = "저장하지 않은 프로그램 이름";
            trayUpdateAll.PerformClick();
            Require(editor.IsVisible && GetField<TextBox>(editor, "NameInput").Text == "저장하지 않은 프로그램 이름" &&
                    !GetField<bool>(window, "batchUpdating") && installCalls.Count == 0,
                "A stale tray action interrupted an open program editor or started an update.");
            RefreshTrayMenu(menu);
            Require(!trayUpdateAll.Enabled, "The tray update-all action was enabled while a program editor was open.");
        }
        finally { editor.Close(); }
        RefreshTrayMenu(menu);
        Require(trayUpdateAll.Enabled, "The tray update-all action stayed disabled after closing the editor.");

        GetMethod(window, "Settings_Click").Invoke(window, [window, new RoutedEventArgs()]);
        app.HideMainWindow();
        trayUpdateAll.PerformClick();
        await WaitUntilAsync(() => GetField<bool>(window, "batchUpdating") && installCalls.Count == 1,
            window.Dispatcher, "tray update-all beginning its first installation");
        Require(window.IsVisible && GetField<TextBlock>(window, "PageTitle").Text == "업데이트" &&
                GetField<TextBox>(window, "SearchBox").Text.Length == 0 &&
                GetField<FrameworkElement>(window, "LibraryView").Visibility == Visibility.Visible &&
                GetField<FrameworkElement>(window, "SettingsView").Visibility == Visibility.Collapsed,
            "The tray update-all action did not restore the library, clear search, and show the update list.");
        Require(!updateAll.IsEnabled && stop.IsEnabled && stop.Visibility == Visibility.Visible,
            "Bulk update controls did not switch to the running state.");
        await CheckMinimumWidthHeaderLayoutAsync(window);

        var callsBeforeReentry = releaseCallOrder.Count;
        await InvokeBatch(window);
        // A stale enabled menu item must still be harmless: the click path rechecks the current state.
        trayUpdateAll.PerformClick();
        RefreshTrayMenu(menu);
        Require(!trayUpdateAll.Enabled, "The tray update-all action remained enabled while a batch was running.");
        trayUpdateAll.PerformClick();
        await (GetMethod(window, "CheckAllAsync").Invoke(window, null) as Task
            ?? throw new InvalidOperationException("CheckAllAsync did not return a Task."));
        GetMethod(window, "AppAction_Click").Invoke(window,
            [new Button { DataContext = thirdItem }, new RoutedEventArgs()]);
        await (GetMethod(window, "EditAppAsync").Invoke(window, [null]) as Task
            ?? throw new InvalidOperationException("EditAppAsync did not return a Task."));
        GetMethod(window, "ImportCatalog_Click").Invoke(window, [window, new RoutedEventArgs()]);
        Require(!(bool)(GetMethod(window, "TryPrepareExit").Invoke(window, null)
            ?? throw new InvalidOperationException("TryPrepareExit returned no result.")),
            "The launcher allowed exit while a bulk update was running.");
        Require(releaseCallOrder.Count == callsBeforeReentry && installCalls.Count == 1,
            "A reentrant update, app action, edit, or catalog import started during the bulk operation.");

        (installGate ?? throw new InvalidOperationException("The fake installation gate was not initialized.")).TrySetResult();
        await WaitUntilAsync(() => !GetField<bool>(window, "batchUpdating"),
            window.Dispatcher, "tray bulk update completion");
        Require(releaseCallOrder.SequenceEqual([
                targetItem.Definition.Id, secondItem.Definition.Id, thirdItem.Definition.Id, launcherItem.Definition.Id
            ], StringComparer.OrdinalIgnoreCase),
            "The bulk update did not check the current app set in stable order with the launcher last.");
        Require(installCalls.SequenceEqual([secondItem.Definition.Id, thirdItem.Definition.Id], StringComparer.OrdinalIgnoreCase),
            "The batch did not skip the newly current app or install the remaining regular apps in order.");
        Require(secondItem.Installed?.Version == "v2.1.0",
            "The batch installed stale metadata instead of the newly available release.");
        Require(maximumConcurrentInstalls == 1,
            $"Bulk installs overlapped; maximum concurrent installations was {maximumConcurrentInstalls}.");
        Require(!GetField<bool>(window, "batchUpdating") && !updateAll.IsEnabled &&
                !stop.IsEnabled && stop.Visibility == Visibility.Collapsed,
            "Bulk controls did not return to the no-updates state after completion.");
        Require(launcherItem.Installed?.Version == launcherVersionBeforeBatch && !launcherItem.HasUpdate,
            "The fake latest launcher check did not safely skip the real self-updater.");
    }

    private async Task CheckBulkFailureStopsRemainderAsync(MainWindow window, List<AppItem> items, AppItem launcherItem)
    {
        var target = SetupBulkFixture(window, items, launcherItem, launcherHasUpdate: true);
        var first = target[0];
        failedInstallIds.Add(first.Definition.Id);
        installDelayMilliseconds = 20;

        await InvokeBatch(window);

        Require(installCalls.SequenceEqual([first.Definition.Id], StringComparer.OrdinalIgnoreCase),
            "A failed app did not stop the rest of the bulk installation sequence.");
        Require(releaseCallOrder.SequenceEqual([first.Definition.Id], StringComparer.OrdinalIgnoreCase),
            "The batch checked later apps or the launcher after an installation failure.");
        Require(first.HasError && !GetField<bool>(window, "batchUpdating"),
            "A bulk installation failure was not reported or left the batch locked.");
        AssertBatchStoppedButtons(window, pendingUpdatesRemain: true);
    }

    private async Task CheckBulkCancellationStopsRemainderAsync(MainWindow window, List<AppItem> items, AppItem launcherItem)
    {
        var target = SetupBulkFixture(window, items, launcherItem, launcherHasUpdate: true);
        var first = target[0];
        installDelayMilliseconds = 0;
        installStarted = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        installGate = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);

        var batchTask = InvokeBatch(window);
        await installStarted.Task.WaitAsync(TimeSpan.FromSeconds(5));
        Require(GetField<bool>(window, "batchUpdating") && first.Busy && first.CanCancel,
            "The first bulk install did not expose a cancellable operation.");
        var stop = GetField<Button>(window, "StopUpdatesButton");
        Require(stop.IsEnabled, "The batch stop button was disabled during a cancellable download.");
        InvokeClickHandler(window, "StopUpdates_Click");
        Require(GetField<bool>(window, "batchCancelRequested") && !stop.IsEnabled,
            "The stop action did not latch cancellation and disable repeat requests.");

        await batchTask;
        Require(installCalls.SequenceEqual([first.Definition.Id], StringComparer.OrdinalIgnoreCase),
            "Cancellation did not stop later app installations.");
        Require(releaseCallOrder.SequenceEqual([first.Definition.Id], StringComparer.OrdinalIgnoreCase),
            "The batch checked later apps or the launcher after cancellation.");
        Require(!GetField<bool>(window, "batchUpdating"),
            "The canceled bulk operation left the batch lock enabled.");
        AssertBatchStoppedButtons(window, pendingUpdatesRemain: true);
    }

    private async Task CheckNoUpdatesDisablesBulkButtonsAsync(App app, MainWindow window, List<AppItem> items, AppItem launcherItem)
    {
        var apps = SetupBulkFixture(window, items, launcherItem, launcherHasUpdate: false, regularAppsHaveUpdates: false);
        InvokeNoArg(window, "RefreshActionAvailability");
        var updateAll = GetField<Button>(window, "UpdateAllButton");
        var stop = GetField<Button>(window, "StopUpdatesButton");
        Require(!updateAll.IsEnabled && !stop.IsEnabled && stop.Visibility == Visibility.Collapsed,
            "Bulk update controls were enabled when every app was already current.");
        var tray = GetField<object>(app, "trayIcon");
        var menu = GetField<System.Windows.Forms.ContextMenuStrip>(tray, "contextMenu");
        var trayUpdateAll = FindTrayUpdateAllItem(menu);
        RefreshTrayMenu(menu);
        Require(!trayUpdateAll.Enabled, "The tray update-all action was enabled when every app was already current.");
        var callsBefore = releaseCallOrder.Count;
        trayUpdateAll.PerformClick();
        await InvokeBatch(window);
        Require(releaseCallOrder.Count == callsBefore && installCalls.Count == 0 && !GetField<bool>(window, "batchUpdating"),
            "Running a no-op bulk update contacted a release source or changed batch state.");
        Require(apps.All(item => !item.HasUpdate), "The latest-version fixture still contained an update.");
    }

    private static System.Windows.Forms.ToolStripMenuItem FindTrayUpdateAllItem(System.Windows.Forms.ContextMenuStrip menu) =>
        menu.Items.OfType<System.Windows.Forms.ToolStripMenuItem>()
            .SingleOrDefault(item => item.Text == "모두 업데이트")
        ?? throw new InvalidOperationException("The tray menu did not contain a '모두 업데이트' action.");

    private static void RefreshTrayMenu(System.Windows.Forms.ContextMenuStrip menu)
    {
        var onOpening = typeof(System.Windows.Forms.ContextMenuStrip).GetMethod("OnOpening",
            BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("ContextMenuStrip.OnOpening was not found.");
        onOpening.Invoke(menu, [new CancelEventArgs()]);
    }

    private async Task CheckBatchStopWaitsForInstallerAsync(MainWindow window, List<AppItem> items, AppItem launcherItem)
    {
        var target = SetupBulkFixture(window, items, launcherItem, launcherHasUpdate: true);
        var first = target[0];
        installDelayMilliseconds = 0;
        installReportsInstallerStarted = true;
        installStarted = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        installGate = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);

        var batchTask = InvokeBatch(window);
        await installStarted.Task.WaitAsync(TimeSpan.FromSeconds(5));
        await WaitUntilAsync(() => first.Busy && !first.CanCancel,
            window.Dispatcher, "non-cancellable installer stage");
        var stop = GetField<Button>(window, "StopUpdatesButton");
        Require(stop.IsEnabled, "The batch could not be stopped while a regular installer was finishing.");
        InvokeClickHandler(window, "StopUpdates_Click");
        Require(GetField<bool>(window, "batchCancelRequested"),
            "Stopping during a non-cancellable installer did not latch cancellation.");
        Require(GetField<bool>(window, "batchUpdating") && first.Busy,
            "The batch did not wait for the non-cancellable installer to finish.");

        (installGate ?? throw new InvalidOperationException("The fake installation gate was not initialized.")).TrySetResult();
        await batchTask;
        Require(installCalls.SequenceEqual([first.Definition.Id], StringComparer.OrdinalIgnoreCase) &&
                first.Installed?.Version == "v2.0.0",
            "The running installer was interrupted instead of completing before the batch stopped.");
        Require(releaseCallOrder.SequenceEqual([first.Definition.Id], StringComparer.OrdinalIgnoreCase),
            "The batch started another update after the non-cancellable installer completed.");
        Require(!GetField<bool>(window, "batchUpdating"),
            "Stopping after a non-cancellable installer left the batch locked.");
        AssertBatchStoppedButtons(window, pendingUpdatesRemain: true);
        installReportsInstallerStarted = false;
    }

    private List<AppItem> SetupBulkFixture(MainWindow window, List<AppItem> items, AppItem launcherItem,
        bool launcherHasUpdate, bool regularAppsHaveUpdates = true)
    {
        releaseCallOrder.Clear();
        releaseCalls.Clear();
        installCalls.Clear();
        failedInstallIds.Clear();
        refreshedInstallations.Clear();
        installDelayMilliseconds = 0;
        installReportsInstallerStarted = false;
        installStarted = null;
        installGate = null;
        maximumConcurrentInstalls = 0;

        var regularDefinitions = new[]
        {
            definitions.Single(definition => definition.Id == "smoke-update-target"),
            definitions.Single(definition => definition.Id == "smoke-update-second"),
            definitions.Single(definition => definition.Id == "smoke-update-cached")
        };
        var regularItems = new List<AppItem>();
        foreach (var definition in regularDefinitions)
        {
            var currentVersion = regularAppsHaveUpdates ? "v1.0.0" : "v2.0.0";
            var latestVersion = "v2.0.0";
            var appItem = new AppItem(definition)
            {
                Installed = MakeInstalled(definition, currentVersion),
                Release = MakeRelease(definition, latestVersion)
            };
            installed[definition.Id] = appItem.Installed!;
            releaseResults[definition.Id] = new ReleaseResult(MakeRelease(definition, latestVersion), IsCached: false);
            regularItems.Add(appItem);
        }

        var launcherInstalledVersion = launcherItem.Installed?.Version
            ?? throw new InvalidOperationException("The launcher fixture has no installed version.");
        var launcherLatest = launcherHasUpdate ? "v999.0.0" : ToReleaseTag(launcherInstalledVersion);
        launcherItem.Release = MakeLauncherRelease(launcherItem.Definition, launcherLatest);
        launcherItem.Error = null;
        launcherItem.Busy = false;
        launcherItem.ActionsBlocked = false;
        releaseResults[launcherItem.Definition.Id] = new ReleaseResult(
            MakeLauncherRelease(launcherItem.Definition, ToReleaseTag(launcherInstalledVersion)), IsCached: false);

        items.Clear();
        // Match the actual library order, so the test proves the launcher is moved to the end.
        items.Add(launcherItem);
        items.AddRange(regularItems);
        SetField(window, "filter", "all");
        GetField<TextBox>(window, "SearchBox").Clear();
        InvokeNoArg(window, "ApplyFilter");
        return regularItems;
    }

    private static void AssertBatchStoppedButtons(MainWindow window, bool pendingUpdatesRemain)
    {
        var updateAll = GetField<Button>(window, "UpdateAllButton");
        var stop = GetField<Button>(window, "StopUpdatesButton");
        Require(updateAll.IsEnabled == pendingUpdatesRemain,
            "The update-all button state did not reflect the remaining pending updates.");
        Require(!stop.IsEnabled && stop.Visibility == Visibility.Collapsed,
            "The stop button remained enabled or visible after the batch ended.");
    }

    private async Task CheckMinimumWidthHeaderLayoutAsync(MainWindow window)
    {
        var originalWidth = window.Width;
        try
        {
            window.WindowState = WindowState.Normal;
            window.Width = window.MinWidth;
            await window.Dispatcher.InvokeAsync(window.UpdateLayout, DispatcherPriority.ApplicationIdle);
            var update = GetField<Button>(window, "UpdateAllButton");
            var refresh = GetField<Button>(window, "RefreshButton");
            var add = FindButton(window, "+ 프로그램 추가")
                ?? throw new InvalidOperationException("Could not find the add-program button in the header.");
            var title = GetField<TextBlock>(window, "PageTitle");
            var previousRight = title.TranslatePoint(new Point(title.ActualWidth, 0), window).X;
            foreach (var button in new[] { update, GetField<Button>(window, "StopUpdatesButton"), refresh, add }
                         .Where(button => button.Visibility == Visibility.Visible))
            {
                var left = button.TranslatePoint(new Point(0, 0), window).X;
                var right = button.TranslatePoint(new Point(button.ActualWidth, 0), window).X;
                Require(previousRight <= left + 0.5 && right <= window.ActualWidth && left >= 0,
                    "A header button overlaps the title/another button or exceeds the minimum-width window.");
                previousRight = right;
            }
        }
        finally
        {
            window.Width = originalWidth;
            await window.Dispatcher.InvokeAsync(window.UpdateLayout, DispatcherPriority.ApplicationIdle);
        }
    }

    private static Button? FindButton(DependencyObject root, string content)
    {
        var count = VisualTreeHelper.GetChildrenCount(root);
        for (var index = 0; index < count; index++)
        {
            var child = VisualTreeHelper.GetChild(root, index);
            if (child is Button button && string.Equals(button.Content as string, content, StringComparison.Ordinal)) return button;
            var nested = FindButton(child, content);
            if (nested is not null) return nested;
        }
        return null;
    }

    private static Task InvokeBatch(MainWindow window) => GetMethod(window, "UpdateAllAsync").Invoke(window, null) as Task
        ?? throw new InvalidOperationException("UpdateAllAsync did not return a Task.");

    private static void InvokeNoArg(MainWindow window, string name) => GetMethod(window, name).Invoke(window, null);

    private static void SetField(object instance, string name, object? value)
    {
        var field = instance.GetType().GetField(name, BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new MissingFieldException(instance.GetType().FullName, name);
        field.SetValue(instance, value);
    }
}
