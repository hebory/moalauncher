using System.Collections.ObjectModel;
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
using CheckBox = System.Windows.Controls.CheckBox;
using Point = System.Windows.Point;
using ProgressBar = System.Windows.Controls.ProgressBar;
using RadioButton = System.Windows.Controls.RadioButton;
using TextBox = System.Windows.Controls.TextBox;

namespace MoaLauncher.App.SmokeTests;

internal static class LibraryViewChecks
{
    private const BindingFlags PrivateInstance = BindingFlags.Instance | BindingFlags.NonPublic;

    internal static async Task RunAsync(MainWindow window, bool leaveInGridView)
    {
        var listButton = GetField<RadioButton>(window, "ListViewButton");
        var gridButton = GetField<RadioButton>(window, "GridViewButton");
        var items = GetField<List<AppItem>>(window, "items");
        var visibleItems = GetField<ObservableCollection<AppItem>>(window, "visibleItems");
        var searchBox = GetField<TextBox>(window, "SearchBox");
        var viewCollection = visibleItems;

        Require(listButton.IsChecked == true && gridButton.IsChecked != true,
            "The initial library view was not the list layout.");
        Require(visibleItems.Count == items.Count,
            "The initial list did not show the full library.");

        var periodicCheck = GetField<CheckBox>(window, "PeriodicCheck");
        var originalPeriodic = periodicCheck.IsChecked;
        var originalSettings = ReadSettings(window);

        gridButton.IsChecked = true;
        await FlushLayoutAsync(window);
        await WaitForSavedViewAsync(window, expected: true);
        Require(gridButton.IsChecked == true && listButton.IsChecked != true,
            "Selecting grid view did not update the selected view control.");
        Require(ReferenceEquals(viewCollection, GetField<ObservableCollection<AppItem>>(window, "visibleItems")),
            "Changing the view replaced the filtered library collection.");

        periodicCheck.IsChecked = false;
        await WaitForSettingsAsync(window, value => !value.PeriodicChecks && value.UseGridView);
        Require(ReadSettings(window).UpdateNotifications == originalSettings.UpdateNotifications &&
                ReadSettings(window).CheckOnStartup == originalSettings.CheckOnStartup,
            "Changing another setting did not preserve the user's existing preferences.");

        listButton.IsChecked = true;
        await FlushLayoutAsync(window);
        await WaitForSavedViewAsync(window, expected: false);
        Require(listButton.IsChecked == true && gridButton.IsChecked != true,
            "Switching back to list view did not select the list layout.");

        periodicCheck.IsChecked = originalPeriodic;
        await WaitForSettingsAsync(window, value => value.PeriodicChecks == originalPeriodic && !value.UseGridView);
        Require(ReadSettings(window).UpdateNotifications == originalSettings.UpdateNotifications &&
                ReadSettings(window).CheckOnStartup == originalSettings.CheckOnStartup,
            "Restoring the other setting changed unrelated preferences.");

        var fixture = CreateLayoutFixture();
        var before = items.ToArray();
        var originalFilter = GetField<string>(window, "filter");
        var originalSearch = searchBox.Text;
        try
        {
            items.AddRange(fixture);
            SetField(window, "filter", "updates");
            searchBox.Text = "그리드 레이아웃 검사";
            InvokeNoArg(window, "ApplyFilter");
            await FlushLayoutAsync(window);
            Require(visibleItems.Count == 1 && ReferenceEquals(visibleItems[0], fixture[0]),
                "The updates filter and search did not find the layout fixture.");

            gridButton.IsChecked = true;
            await FlushLayoutAsync(window);
            await WaitForSavedViewAsync(window, expected: true);
            Require(GetField<string>(window, "filter") == "updates" && searchBox.Text == "그리드 레이아웃 검사" &&
                    visibleItems.Count == 1 && ReferenceEquals(visibleItems[0], fixture[0]),
                "Switching views reset the update filter, search, or filtered item.");
            Require(ReadSettings(window).UseGridView,
                "Grid view was not persisted after changing views with an active filter.");
            AssertCardOperationsVisible(window, fixture[0]);

            searchBox.Clear();
            InvokeNoArg(window, "ApplyFilter");
            await FlushLayoutAsync(window);
            var library = GetField<FrameworkElement>(window, "LibraryView");
            var originalWidth = window.Width;
            try
            {
                window.WindowState = WindowState.Normal;
                foreach (var width in new[] { window.MinWidth, 960d })
                {
                    window.Width = width;
                    await FlushLayoutAsync(window);
                    AssertCardsFit(window, library, visibleItems);
                    var cards = visibleItems.Select(item => GetCard(window, item)).ToArray();
                    var columns = CountFirstRowColumns(window, cards);
                    var expectedColumns = width <= 760 ? 2 : 3;
                    Require(columns == expectedColumns,
                        $"Grid layout used {columns} columns at window width {width}; expected {expectedColumns}.");
                }
            }
            finally
            {
                window.Width = originalWidth;
                await FlushLayoutAsync(window);
            }

            listButton.IsChecked = true;
            await FlushLayoutAsync(window);
            Require(GetField<string>(window, "filter") == "updates" && visibleItems.Contains(fixture[0]),
                "Returning to list view lost the active update-filter result.");
        }
        finally
        {
            items.Clear();
            items.AddRange(before);
            searchBox.Text = originalSearch;
            SetField(window, "filter", originalFilter);
            InvokeNoArg(window, "ApplyFilter");
            if (leaveInGridView) gridButton.IsChecked = true;
            else listButton.IsChecked = true;
            await FlushLayoutAsync(window);
            await WaitForSavedViewAsync(window, leaveInGridView);
        }

        Console.WriteLine("PASS: list/grid switching, persisted view preference, filter/search preservation, responsive grid bounds, and card progress/error/cancel actions.");
    }

    private static List<AppItem> CreateLayoutFixture()
    {
        var items = new List<AppItem>();
        for (var index = 0; index < 8; index++)
        {
            var definition = new AppDefinition
            {
                Id = $"grid-layout-smoke-{index}",
                Name = index == 0 ? "그리드 레이아웃 검사" : $"그리드 앱 {index}",
                Description = "그리드 배치 및 상태 표시 검사",
                Category = "테스트",
                Monogram = "G",
                Repository = $"smoke/grid-{index}",
                Packages = [new PackageDefinition { Kind = "zip", AssetPattern = "app.zip", ExecutablePath = "App.exe" }]
            };
            var item = new AppItem(definition)
            {
                Installed = new InstalledApp(definition.Id, "1.0.0", "App.exe", "smoke", "zip", DateTimeOffset.UtcNow),
                Release = new AppRelease("v2.0.0", "2.0.0", "검사 릴리스", new Uri($"https://github.com/{definition.Repository}/releases/tag/v2.0.0"),
                    DateTimeOffset.UtcNow, [])
            };
            items.Add(item);
        }

        var active = items[0];
        active.Busy = true;
        active.ProgressText = "다운로드 중 · 42%";
        active.Percent = 42;
        active.CanCancel = true;
        active.Error = "예시 네트워크 오류 안내";
        return items;
    }

    private static void AssertCardOperationsVisible(MainWindow window, AppItem item)
    {
        var card = GetCard(window, item);
        var progress = FindVisualChild<ProgressBar>(card, progress => progress.Visibility == Visibility.Visible);
        Require(progress is not null && Math.Abs(progress.Value - 42) < 0.1,
            "The grid card did not display the AppItem download progress.");
        var cancel = FindVisualChild<Button>(card, button => (button.Content as string) == "다운로드 취소");
        Require(cancel is { Visibility: Visibility.Visible, IsEnabled: true },
            "The grid card did not expose its enabled download-cancel action.");
        var error = FindVisualChild<TextBlock>(card, text => text.Text == item.Error && text.Visibility == Visibility.Visible);
        Require(error is not null,
            "The grid card did not display the AppItem error message.");
        var action = FindVisualChild<Button>(card, button => button.Content as string == item.ActionText);
        Require(action is { IsEnabled: false },
            "The grid card did not disable its main action while the AppItem was busy.");
    }

    private static void AssertCardsFit(MainWindow window, FrameworkElement library, IReadOnlyList<AppItem> items)
    {
        var left = library.TranslatePoint(new Point(0, 0), window).X;
        var right = left + library.ActualWidth;
        var cards = items.Select(item => GetCard(window, item)).ToArray();
        foreach (var card in cards)
        {
            var origin = card.TranslatePoint(new Point(0, 0), window);
            Require(origin.X >= left - 0.5 && origin.X + card.ActualWidth <= right + 0.5,
                "A grid card exceeded the available library width.");
            Require(card.ActualWidth >= 267,
                "A grid card became narrower than the supported minimum cell width.");
        }

        var panel = FindVisualChild<WrapPanel>(GetField<ItemsControl>(window, "AppCards"), _ => true);
        var columns = CountFirstRowColumns(window, cards);
        Require(panel is { ActualWidth: > 0, ItemWidth: > 0 } &&
                Math.Abs(panel.ItemWidth * columns - panel.ActualWidth) <= columns + 0.5,
            "The grid did not size its fixed-width cells to the available panel width.");
    }

    private static int CountFirstRowColumns(MainWindow window, IReadOnlyList<FrameworkElement> cards)
    {
        var firstTop = cards[0].TranslatePoint(new Point(0, 0), window).Y;
        return cards.Count(card => Math.Abs(card.TranslatePoint(new Point(0, 0), window).Y - firstTop) < 1);
    }

    private static FrameworkElement GetCard(MainWindow window, AppItem item)
    {
        var container = GetField<ItemsControl>(window, "AppCards").ItemContainerGenerator.ContainerFromItem(item)
            ?? throw new InvalidOperationException($"The {item.Name} grid card container was not generated.");
        return FindVisualChild<Border>(container, border =>
                   ReferenceEquals(border.DataContext, item) && border.CornerRadius.TopLeft == 12)
               ?? throw new InvalidOperationException($"Could not find the {item.Name} program card.");
    }

    private static T? FindVisualChild<T>(DependencyObject root, Func<T, bool> predicate) where T : DependencyObject
    {
        if (root is T match && predicate(match)) return match;
        for (var index = 0; index < VisualTreeHelper.GetChildrenCount(root); index++)
        {
            var found = FindVisualChild<T>(VisualTreeHelper.GetChild(root, index), predicate);
            if (found is not null) return found;
        }
        return null;
    }

    private static async Task WaitForSavedViewAsync(MainWindow window, bool expected)
    {
        await WaitForSettingsAsync(window, settings => settings.UseGridView == expected);
    }

    private static async Task WaitForSettingsAsync(MainWindow window, Func<LauncherSettings, bool> predicate)
    {
        var gate = GetField<SemaphoreSlim>(window, "settingsGate");
        if (!await gate.WaitAsync(TimeSpan.FromSeconds(5)))
            throw new TimeoutException("The settings file remained busy while checking the saved library view.");
        try
        {
            Require(predicate(ReadSettings(window)), "The selected library view/settings were not saved.");
        }
        finally { gate.Release(); }
    }

    private static LauncherSettings ReadSettings(MainWindow window) =>
        JsonSerializer.Deserialize<LauncherSettings>(File.ReadAllText(SettingsPath(window)), CatalogService.JsonOptions)
        ?? throw new InvalidDataException("The smoke test settings were empty.");

    private static string SettingsPath(MainWindow window)
        => Path.Combine(App.DataDirectory, "settings.json");

    private static async Task FlushLayoutAsync(MainWindow window)
    {
        await window.Dispatcher.InvokeAsync(() =>
        {
            window.UpdateLayout();
            GetField<ItemsControl>(window, "AppCards").UpdateLayout();
        }, DispatcherPriority.ApplicationIdle);
    }

    private static T GetField<T>(object instance, string name) =>
        (T)(instance.GetType().GetField(name, PrivateInstance)?.GetValue(instance)
            ?? throw new MissingFieldException(instance.GetType().FullName, name));

    private static void SetField(object instance, string name, object? value)
    {
        var field = instance.GetType().GetField(name, PrivateInstance)
            ?? throw new MissingFieldException(instance.GetType().FullName, name);
        field.SetValue(instance, value);
    }

    private static void InvokeNoArg(object instance, string name) =>
        (instance.GetType().GetMethod(name, PrivateInstance)
         ?? throw new MissingMethodException(instance.GetType().FullName, name)).Invoke(instance, null);

    private static void Require(bool value, string message)
    {
        if (!value) throw new InvalidOperationException(message);
    }
}
