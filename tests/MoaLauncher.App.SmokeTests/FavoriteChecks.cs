using System.Collections.ObjectModel;
using System.IO;
using System.Reflection;
using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Media;
using System.Windows.Threading;
using MoaLauncher.App;
using MoaLauncher.Core;
using TextBox = System.Windows.Controls.TextBox;
using RadioButton = System.Windows.Controls.RadioButton;
using ButtonBase = System.Windows.Controls.Primitives.ButtonBase;

namespace MoaLauncher.App.SmokeTests;

internal static class FavoriteChecks
{
    private const BindingFlags PrivateInstance = BindingFlags.Instance | BindingFlags.NonPublic;

    internal static async Task RunAsync(MainWindow window)
    {
        var items = Field<List<AppItem>>(window, "items");
        var visible = Field<ObservableCollection<AppItem>>(window, "visibleItems");
        var search = Field<TextBox>(window, "SearchBox");
        var list = Field<RadioButton>(window, "ListViewButton");
        var grid = Field<RadioButton>(window, "GridViewButton");
        var originalItems = items.ToArray();
        var originalSettings = Settings(window);
        var originalFilter = Field<string>(window, "filter");
        var originalSearch = search.Text;
        var originalCatalogPath = Path.Combine(App.DataDirectory, "catalog.json");
        var originalCatalog = File.Exists(originalCatalogPath) ? await File.ReadAllBytesAsync(originalCatalogPath) : null;

        var candidateIds = items.Where(item => !item.IsLauncher).Take(2).Select(item => item.Definition.Id).ToArray();
        if (candidateIds.Length < 2) throw new InvalidOperationException("Favorite smoke check needs two regular catalog apps.");
        var candidateNames = candidateIds.Select(id => items.Single(item => item.Definition.Id == id).Name).ToArray();
        var privateDefinition = new AppDefinition
        {
            Id = "favorite-private-smoke", Name = "개인 즐겨찾기 검사", Repository = "smoke/private-favorite",
            IsPrivate = true, Packages = [new PackageDefinition { Kind = "zip", AssetPattern = "private.zip", ExecutablePath = "Private.exe" }]
        };
        var privateItem = new AppItem(privateDefinition);
        try
        {
            SetField(window, "settings", originalSettings with { FavoriteAppIds = [] });
            foreach (var item in items) item.IsFavorite = false;
            SetField(window, "filter", "all");
            items.Add(privateItem);
            Invoke(window, "ApplyFilter");
            await InvokeTask(window, "SaveSettingsAsync");
            await FlushAsync(window);
            foreach (var useGrid in new[] { false, true })
            {
                if (!items.Contains(privateItem))
                {
                    privateItem.IsFavorite = Settings(window).FavoriteAppIds.Contains(privateDefinition.Id);
                    items.Add(privateItem);
                    Invoke(window, "ApplyFilter");
                    await FlushAsync(window);
                }
                if (useGrid) grid.IsChecked = true;
                else list.IsChecked = true;
                await FlushAsync(window);
                foreach (var id in candidateIds)
                {
                    var item = items.Single(current => current.Definition.Id == id);
                    SetFavorite(window, item, enabled: true);
                    await WaitForFavoritesAsync(window, ids => ids.Contains(item.Definition.Id));
                    Require(item.IsFavorite, "The favorite action did not update the card state.");
                }
                SetFavorite(window, privateItem, enabled: true);
                await WaitForFavoritesAsync(window, ids => ids.Contains(privateDefinition.Id));
                Require(privateItem.IsFavorite, "A private personal app could not be favorited.");

                Invoke(window, "Favorites_Click", window, new RoutedEventArgs());
                await FlushAsync(window);
                Require(Field<string>(window, "filter") == "favorites" && visible.Count == 3 &&
                        candidateIds.All(id => visible.Any(item => item.Definition.Id == id)) && visible.Contains(privateItem),
                    "Favorites view did not show the favorited apps in the selected card layout.");
                var firstCandidate = items.Single(item => item.Definition.Id == candidateIds[0]);
                SetFavorite(window, firstCandidate, enabled: false);
                await FlushAsync(window);
                await WaitForFavoritesAsync(window, ids => !ids.Contains(firstCandidate.Definition.Id));
                Require(!visible.Contains(firstCandidate), "Removing a favorite did not remove its card from the favorites view.");
                Invoke(window, "All_Click", window, new RoutedEventArgs());
                await FlushAsync(window);
                SetFavorite(window, firstCandidate, enabled: true);
                await FlushAsync(window);
                await WaitForFavoritesAsync(window, ids => ids.Contains(firstCandidate.Definition.Id));
                Invoke(window, "Favorites_Click", window, new RoutedEventArgs());
                await FlushAsync(window);
                Require(visible.Contains(firstCandidate), "Adding a favorite again did not restore its card in the favorites view.");
                search.Text = candidateNames[0];
                Invoke(window, "ApplyFilter");
                Require(visible.Count == 1 && ReferenceEquals(visible[0], firstCandidate),
                    "Search and favorites filters were not combined.");
                search.Text = "no-favorite-match-smoke";
                Invoke(window, "ApplyFilter");
                Require(visible.Count == 0 && Field<TextBlock>(window, "EmptyText").Text.Contains("검색", StringComparison.Ordinal),
                    "A search without favorite matches did not produce the search empty state.");

                search.Clear();
                Invoke(window, "ApplyFilter");
                Require(visible.Count == 3, "Clearing search did not restore the favorite list.");
                var persisted = Settings(window);
                Require(persisted.FavoriteAppIds.Count == 3 && candidateIds.All(persisted.FavoriteAppIds.Contains) &&
                        persisted.FavoriteAppIds.Contains(privateDefinition.Id),
                    "Favorites did not persist for both public and private apps.");

                await InvokeTask(window, "LoadCatalogAsync");
                var reloaded = Field<List<AppItem>>(window, "items");
                Require(candidateIds.All(id => reloaded.Single(current => current.Definition.Id == id).IsFavorite),
                    "Saved favorites were lost when the app catalog was reloaded.");
                privateItem.IsFavorite = Settings(window).FavoriteAppIds.Contains(privateDefinition.Id);
                reloaded.Add(privateItem);
                Invoke(window, "ApplyFilter");
                await FlushAsync(window);
                Require(originalCatalog is null
                        ? !File.Exists(originalCatalogPath)
                        : File.ReadAllBytes(originalCatalogPath).SequenceEqual(originalCatalog),
                    "Favoriting an app unexpectedly modified the personal app catalog.");
            }

            foreach (var id in candidateIds.Append(privateDefinition.Id))
            {
                SetFavorite(window, Field<List<AppItem>>(window, "items").Single(item => item.Definition.Id == id), enabled: false);
                await FlushAsync(window);
            }
            await WaitForFavoritesAsync(window, ids => ids.Count == 0);
            Invoke(window, "Favorites_Click", window, new RoutedEventArgs());
            Require(visible.Count == 0 && Field<string>(window, "filter") == "favorites" &&
                    Field<TextBlock>(window, "EmptyText").Text.Contains("즐겨찾", StringComparison.Ordinal),
                "An empty favorites view did not show its empty state.");
            Invoke(window, "All_Click", window, new RoutedEventArgs());
            Require(visible.Count == originalItems.Length + 1,
                "Returning to all apps after clearing favorites did not restore the library.");
        }
        finally
        {
            items.Clear();
            items.AddRange(originalItems);
            foreach (var item in originalItems) item.IsFavorite = originalSettings.FavoriteAppIds.Contains(item.Definition.Id);
            SetField(window, "settings", originalSettings);
            search.Text = originalSearch;
            SetField(window, "filter", originalFilter);
            Invoke(window, "ApplyViewMode");
            await InvokeTask(window, "SaveSettingsAsync");
            Invoke(window, "ApplyFilter");
            await FlushAsync(window);
        }

        Console.WriteLine("PASS: favorite toggles in list/grid, persisted private/public favorites, favorite/search filtering, empty state, and catalog isolation.");
    }

    private static void SetFavorite(MainWindow window, AppItem item, bool enabled)
    {
        var container = Field<ItemsControl>(window, "AppCards").ItemContainerGenerator.ContainerFromItem(item)
            ?? throw new InvalidOperationException($"The {item.Name} favorite card was not generated.");
        var button = FindChild<ToggleButton>(container, candidate => candidate.Name == "FavoriteButton")
            ?? throw new InvalidOperationException($"The {item.Name} card did not expose its favorite control.");
        Require(button.IsChecked == item.IsFavorite, "The favorite card control and item state were out of sync.");
        if (item.IsFavorite == enabled) return;
        button.RaiseEvent(new RoutedEventArgs(ButtonBase.ClickEvent, button));
        Require(item.IsFavorite == enabled, "The favorite card click did not update the app state.");
    }

    private static async Task WaitForFavoritesAsync(MainWindow window, Func<IReadOnlyList<string>, bool> predicate)
    {
        var deadline = DateTime.UtcNow.AddSeconds(5);
        while (!predicate(ReadSavedSettings(window).FavoriteAppIds))
        {
            if (DateTime.UtcNow >= deadline) throw new TimeoutException("Favorite settings were not saved.");
            await window.Dispatcher.InvokeAsync(() => { }, DispatcherPriority.ApplicationIdle);
            await Task.Delay(10);
        }
    }

    private static LauncherSettings Settings(object window) => Field<LauncherSettings>(window, "settings");
    private static LauncherSettings ReadSavedSettings(MainWindow window) =>
        JsonSerializer.Deserialize<LauncherSettings>(File.ReadAllText(Path.Combine(App.DataDirectory, "settings.json")), CatalogService.JsonOptions)
        ?? throw new InvalidDataException("The favorite settings file was empty.");
    private static T Field<T>(object instance, string name) => (T)(instance.GetType().GetField(name, PrivateInstance)?.GetValue(instance)
        ?? throw new MissingFieldException(name));
    private static void SetField(object instance, string name, object value) =>
        (instance.GetType().GetField(name, PrivateInstance) ?? throw new MissingFieldException(name)).SetValue(instance, value);
    private static void Invoke(object instance, string name, params object[] args) =>
        (instance.GetType().GetMethod(name, PrivateInstance) ?? throw new MissingMethodException(name)).Invoke(instance, args);
    private static async Task InvokeTask(object instance, string name)
    {
        var task = (Task)(instance.GetType().GetMethod(name, PrivateInstance)?.Invoke(instance, null)
            ?? throw new MissingMethodException(name));
        await task;
    }
    private static async Task FlushAsync(MainWindow window) => await window.Dispatcher.InvokeAsync(() => window.UpdateLayout(), DispatcherPriority.ApplicationIdle);
    private static T? FindChild<T>(DependencyObject root, Func<T, bool> predicate) where T : DependencyObject
    {
        if (root is T match && predicate(match)) return match;
        for (var index = 0; index < VisualTreeHelper.GetChildrenCount(root); index++)
            if (FindChild<T>(VisualTreeHelper.GetChild(root, index), predicate) is { } child) return child;
        return null;
    }
    private static void Require(bool condition, string message) { if (!condition) throw new InvalidOperationException(message); }
}
