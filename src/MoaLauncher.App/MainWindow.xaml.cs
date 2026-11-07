using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Net.Http;
using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Threading;
using Microsoft.Win32;
using MoaLauncher.Core;

namespace MoaLauncher.App;

public partial class MainWindow : Window
{
    private readonly HttpClient http = new(new HttpClientHandler { AllowAutoRedirect = false }) { Timeout = TimeSpan.FromSeconds(45) };
    private readonly IReleaseService releases;
    private readonly IInstallationService installations;
    private readonly List<AppItem> items = [];
    private readonly ObservableCollection<AppItem> visibleItems = [];
    private readonly Dictionary<string, CancellationTokenSource> operations = [];
    private readonly DispatcherTimer timer = new() { Interval = TimeSpan.FromMinutes(30) };
    private AppCatalog overrides = new();
    private LauncherSettings settings = new();
    private bool ready;
    private bool checking;
    private readonly SemaphoreSlim settingsGate = new(1, 1);
    private readonly CancellationTokenSource lifetime = new();
    private string filter = "all";
    private string CatalogPath => Path.Combine(App.DataDirectory, "catalog.json");
    private string SettingsPath => Path.Combine(App.DataDirectory, "settings.json");

    public MainWindow()
    {
        InitializeComponent();
        releases = new GitHubReleaseService(http, Path.Combine(App.DataDirectory, "cache"));
        installations = new InstallationService(http, App.DataDirectory);
        AppCards.ItemsSource = visibleItems;
        DataPathText.Text = App.DataDirectory;
        Loaded += async (_, _) => await InitializeAsync();
        Closing += OnClosing;
        Closed += (_, _) => { timer.Stop(); lifetime.Cancel(); http.Dispose(); };
        timer.Tick += async (_, _) => { if (settings.PeriodicChecks && !checking && operations.Count == 0) await CheckAllAsync(); };
    }

    private async Task InitializeAsync()
    {
        try
        {
            if (File.Exists(SettingsPath))
            {
                try { settings = JsonSerializer.Deserialize<LauncherSettings>(await File.ReadAllTextAsync(SettingsPath), CatalogService.JsonOptions) ?? new(); }
                catch (Exception ex) when (ex is JsonException or IOException) { App.Log(ex); settings = new(); }
            }
            StartupCheck.IsChecked = settings.CheckOnStartup;
            PeriodicCheck.IsChecked = settings.PeriodicChecks;
            await LoadCatalogAsync();
            ready = true;
            timer.Start();
            if (settings.CheckOnStartup) await CheckAllAsync();
        }
        catch (Exception ex) { ShowError(ex); }
    }

    private async Task LoadCatalogAsync()
    {
        var bundled = await CatalogService.LoadAsync(Path.Combine(AppContext.BaseDirectory, "catalog", "apps.json"));
        if (File.Exists(CatalogPath))
        {
            try { overrides = await CatalogService.LoadAsync(CatalogPath); }
            catch (Exception ex) { ShowError(new InvalidDataException("저장한 카탈로그를 읽지 못해 기본 목록을 표시합니다. 데이터 폴더의 catalog.json을 확인하세요.", ex)); }
        }
        var merged = bundled.Apps.ToDictionary(a => a.Id, StringComparer.OrdinalIgnoreCase);
        foreach (var app in overrides.Apps) merged[app.Id] = app;
        items.Clear();
        foreach (var definition in merged.Values)
        {
            var item = new AppItem(definition);
            try { item.Installed = await installations.FindInstalledAsync(definition); }
            catch (Exception ex) { item.Error = ex.Message; App.Log(ex); }
            items.Add(item);
        }
        DetailsPanel.Visibility = Visibility.Collapsed;
        ApplyFilter();
    }

    private void ApplyFilter()
    {
        if (AppCards is null) return;
        var search = SearchBox.Text.Trim();
        visibleItems.Clear();
        foreach (var item in items.Where(i => (filter != "installed" || i.IsInstalled) && (filter != "updates" || i.HasUpdate)))
            if ($"{item.Name} {item.Description} {item.Repository}".Contains(search, StringComparison.OrdinalIgnoreCase)) visibleItems.Add(item);
        TotalCount.Text = items.Count.ToString();
        InstalledCount.Text = items.Count(i => i.IsInstalled).ToString();
        UpdateCount.Text = items.Count(i => i.HasUpdate).ToString();
        EmptyText.Text = filter == "updates" ? "현재 확인된 업데이트가 없습니다. ‘업데이트 확인’으로 최신 상태를 확인하세요." : "검색 조건에 맞는 프로그램이 없습니다.";
        EmptyText.Visibility = visibleItems.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        AllFilter.Background = new SolidColorBrush((Color)ColorConverter.ConvertFromString(filter == "all" ? "#354339" : "#151923"));
        InstalledFilter.Background = new SolidColorBrush((Color)ColorConverter.ConvertFromString(filter == "installed" ? "#354339" : "#151923"));
    }

    private async Task CheckOneAsync(AppItem item, CancellationToken ct = default)
    {
        item.Error = null;
        item.ProgressText = "새 버전 확인 중…";
        try
        {
            item.Installed = await installations.FindInstalledAsync(item.Definition, ct);
            if (!item.NeedsConnection)
            {
                var result = await releases.GetLatestAsync(item.Definition, ct);
                item.Release = result.Release;
                item.IsCached = result.IsCached;
                if (result.IsCached) item.Error = "GitHub에 연결하지 못해 이전 확인 결과를 표시합니다.";
                else
                {
                    var package = ReleaseRules.SelectPackage(item.Definition);
                    _ = ReleaseRules.SelectAsset(result.Release, package);
                }
            }
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested) { throw; }
        catch (Exception ex) { item.Error = ex.Message; App.Log(ex); }
        finally { item.Refresh(); }
    }

    private async Task CheckAllAsync()
    {
        if (checking || operations.Count > 0) return;
        checking = true;
        RefreshButton.IsEnabled = false;
        StatusText.Text = "프로그램의 새 버전을 확인하고 있습니다…";
        try
        {
            // Sequential requests respect GitHub's anonymous rate limit.
            foreach (var item in items.ToArray())
            {
                if (item.Busy) continue;
                item.Busy = true;
                try { await CheckOneAsync(item, lifetime.Token); }
                finally { item.Busy = false; }
            }
            LastCheckedText.Text = $"마지막 확인 {DateTime.Now:HH:mm}";
            var errors = items.Count(i => i.HasError);
            StatusText.Text = errors > 0 ? $"{errors}개 앱의 확인 결과를 살펴보세요. 앱 카드에 원인을 표시했습니다." : items.All(i => i.NeedsConnection) ? "저장소를 연결하면 최신 릴리스를 확인할 수 있습니다." : $"확인 완료 · 업데이트 가능한 프로그램 {items.Count(i => i.HasUpdate)}개";
            ApplyFilter();
        }
        catch (OperationCanceledException) when (lifetime.IsCancellationRequested) { }
        finally { checking = false; RefreshButton.IsEnabled = operations.Count == 0; }
    }

    private async void AppAction_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is not AppItem item || item.Busy) return;
        if (item.NeedsConnection) { await EditAppAsync(item); return; }
        if (item.IsInstalled && !item.HasUpdate) { LaunchItem(item); return; }
        if (item.Release is null)
        {
            item.Busy = true;
            try { await CheckOneAsync(item, lifetime.Token); }
            catch (OperationCanceledException) when (lifetime.IsCancellationRequested) { }
            finally { item.Busy = false; ApplyFilter(); }
            return;
        }
        using var cancellation = new CancellationTokenSource();
        operations[item.Definition.Id] = cancellation;
        RefreshButton.IsEnabled = false;
        item.CanCancel = true;
        item.Busy = true;
        item.Error = null;
        item.Percent = 0;
        try
        {
            // Do not install using stale cache metadata when the source cannot be reached.
            item.ProgressText = "배포 정보 확인 중…";
            var fresh = await releases.GetLatestAsync(item.Definition, cancellation.Token);
            if (fresh.IsCached) throw new IOException("인터넷 연결을 확인한 뒤 설치를 다시 시도하세요.");
            item.Release = fresh.Release;
            var progress = new Progress<InstallProgress>(p =>
            {
                if (p.Stage.StartsWith("설치 프로그램", StringComparison.Ordinal) || p.Stage.StartsWith("설치 중", StringComparison.Ordinal)) item.CanCancel = false;
                item.ProgressText = p.Stage;
                item.Percent = p.Percent;
                item.Refresh();
            });
            item.Installed = await installations.InstallAsync(item.Definition, fresh.Release, progress, cancellation.Token);
            item.IsCached = false;
            StatusText.Text = $"{item.Name} {item.Installed.Version} 설치를 완료했습니다.";
        }
        catch (OperationCanceledException) { StatusText.Text = $"{item.Name} 작업을 취소했습니다."; }
        catch (Exception ex) { item.Error = ex.Message; ShowError(ex); }
        finally { operations.Remove(item.Definition.Id); item.CanCancel = false; item.Busy = false; RefreshButton.IsEnabled = operations.Count == 0 && !checking; ApplyFilter(); }
    }

    private void LaunchItem(AppItem item)
    {
        try
        {
            if (item.Installed is null) return;
            installations.Launch(item.Installed);
            StatusText.Text = $"{item.Name} 실행을 요청했습니다.";
        }
        catch (Exception ex) { item.Error = ex.Message; ShowError(ex); }
    }

    private void Cancel_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is AppItem item && operations.TryGetValue(item.Definition.Id, out var cancellation)) cancellation.Cancel();
    }
    private async void Refresh_Click(object sender, RoutedEventArgs e) => await CheckAllAsync();
    private void Search_Changed(object sender, TextChangedEventArgs e) { if (ready) ApplyFilter(); }
    private void All_Click(object sender, RoutedEventArgs e) { ShowLibrary(); PageTitle.Text = "내 프로그램"; filter = "all"; ApplyFilter(); }
    private void Installed_Click(object sender, RoutedEventArgs e) { ShowLibrary(); PageTitle.Text = "내 프로그램"; filter = "installed"; ApplyFilter(); }
    private void Library_Click(object sender, RoutedEventArgs e) { ShowLibrary(); filter = "all"; PageTitle.Text = "내 프로그램"; ApplyFilter(); }
    private void Updates_Click(object sender, RoutedEventArgs e) { ShowLibrary(); SelectNavigation(UpdatesNav); filter = "updates"; PageTitle.Text = "업데이트"; ApplyFilter(); }
    private void Settings_Click(object sender, RoutedEventArgs e) { LibraryView.Visibility = Visibility.Collapsed; SettingsView.Visibility = Visibility.Visible; SelectNavigation(SettingsNav); }
    private void ShowLibrary() { LibraryView.Visibility = Visibility.Visible; SettingsView.Visibility = Visibility.Collapsed; SelectNavigation(LibraryNav); }
    private void SelectNavigation(Button selected)
    {
        foreach (var button in new[] { LibraryNav, UpdatesNav, SettingsNav })
            button.Background = button == selected ? new SolidColorBrush(Color.FromRgb(43, 53, 64)) : Brushes.Transparent;
    }
    private void Details_Click(object sender, RoutedEventArgs e) { DetailsPanel.DataContext = ((FrameworkElement)sender).DataContext; DetailsPanel.Visibility = Visibility.Visible; DetailsPanel.BringIntoView(); }
    private void CloseDetails_Click(object sender, RoutedEventArgs e) => DetailsPanel.Visibility = Visibility.Collapsed;
    private void Launch_Click(object sender, RoutedEventArgs e) { if (DetailsPanel.DataContext is AppItem item) LaunchItem(item); }

    private void OpenRepository_Click(object sender, RoutedEventArgs e)
    {
        if (DetailsPanel.DataContext is not AppItem item || item.NeedsConnection) return;
        OpenPath($"https://github.com/{item.Repository}/releases");
    }
    private void OpenInstallFolder_Click(object sender, RoutedEventArgs e)
    {
        if (DetailsPanel.DataContext is AppItem { Installed: not null } item) OpenPath(item.Installed.InstallDirectory);
    }
    private void OpenDataFolder_Click(object sender, RoutedEventArgs e) => OpenPath(App.DataDirectory);
    private void OpenPath(string path)
    {
        try { Process.Start(new ProcessStartInfo(path) { UseShellExecute = true }); }
        catch (Exception ex) { ShowError(ex); }
    }

    private async void AddApp_Click(object sender, RoutedEventArgs e) => await EditAppAsync(null);
    private async void EditApp_Click(object sender, RoutedEventArgs e) => await EditAppAsync(DetailsPanel.DataContext as AppItem);
    private async Task EditAppAsync(AppItem? item)
    {
        if (checking || items.Any(i => i.Busy)) { StatusText.Text = "진행 중인 작업이 끝나면 프로그램 설정을 변경할 수 있습니다."; return; }
        var dialog = new AppEditorWindow(item?.Definition) { Owner = this };
        if (dialog.ShowDialog() != true || dialog.Result is null) return;
        if (item is null && items.Any(i => i.Definition.Id.Equals(dialog.Result.Id, StringComparison.OrdinalIgnoreCase)))
        { StatusText.Text = "이미 등록된 ID입니다. 해당 앱의 ‘프로그램 설정’에서 변경하세요."; return; }
        try
        {
            var updated = overrides.Apps.Where(a => !a.Id.Equals(dialog.Result.Id, StringComparison.OrdinalIgnoreCase)).Append(dialog.Result).ToList();
            await CatalogService.SaveAsync(CatalogPath, new AppCatalog { Apps = updated });
            await LoadCatalogAsync();
            StatusText.Text = $"{dialog.Result.Name} 프로그램 설정을 저장했습니다.";
            ShowLibrary();
            PageTitle.Text = "내 프로그램";
            filter = "all";
            SearchBox.Clear();
            var added = items.First(i => i.Definition.Id == dialog.Result.Id);
            added.Busy = true;
            try { await CheckOneAsync(added, lifetime.Token); }
            catch (OperationCanceledException) when (lifetime.IsCancellationRequested) { }
            finally { added.Busy = false; ApplyFilter(); }
        }
        catch (Exception ex) { ShowError(ex); }
    }

    private async void Setting_Changed(object sender, RoutedEventArgs e)
    {
        if (!ready) return;
        settings = new LauncherSettings { CheckOnStartup = StartupCheck.IsChecked == true, PeriodicChecks = PeriodicCheck.IsChecked == true };
        await settingsGate.WaitAsync();
        try
        {
            var temp = SettingsPath + ".tmp";
            await File.WriteAllTextAsync(temp, JsonSerializer.Serialize(settings, CatalogService.JsonOptions));
            File.Move(temp, SettingsPath, true);
            StatusText.Text = "설정을 저장했습니다.";
        }
        catch (Exception ex) { ShowError(ex); }
        finally { settingsGate.Release(); }
    }

    private async void ImportCatalog_Click(object sender, RoutedEventArgs e)
    {
        if (checking || items.Any(i => i.Busy)) { StatusText.Text = "진행 중인 작업이 끝난 뒤 카탈로그를 가져오세요."; return; }
        var dialog = new OpenFileDialog { Filter = "앱 카탈로그 (*.json)|*.json" };
        if (dialog.ShowDialog(this) != true) return;
        try
        {
            var imported = await CatalogService.LoadAsync(dialog.FileName);
            var summary = string.Join("\n", imported.Apps.Take(10).Select(a => $"• {a.Name} — {a.Repository}"));
            if (MessageBox.Show(this, $"다음 배포자의 프로그램 {imported.Apps.Count}개를 등록합니다.\n같은 ID의 프로그램 설정은 변경됩니다.\n\n{summary}", "카탈로그 가져오기", MessageBoxButton.OKCancel, MessageBoxImage.Information) != MessageBoxResult.OK) return;
            var merged = overrides.Apps.ToDictionary(a => a.Id, StringComparer.OrdinalIgnoreCase);
            foreach (var app in imported.Apps) merged[app.Id] = app;
            await CatalogService.SaveAsync(CatalogPath, new AppCatalog { Apps = merged.Values.ToList() });
            await LoadCatalogAsync();
            StatusText.Text = $"프로그램 {imported.Apps.Count}개를 가져왔습니다.";
        }
        catch (Exception ex) { ShowError(ex); }
    }
    private async void ExportCatalog_Click(object sender, RoutedEventArgs e)
    {
        var dialog = new SaveFileDialog { Filter = "앱 카탈로그 (*.json)|*.json", FileName = "moa-apps.json" };
        if (dialog.ShowDialog(this) != true) return;
        try { await CatalogService.SaveAsync(dialog.FileName, new AppCatalog { Apps = items.Select(i => i.Definition).ToList() }); StatusText.Text = "카탈로그를 내보냈습니다."; }
        catch (Exception ex) { ShowError(ex); }
    }
    private void ShowError(Exception ex) { App.Log(ex); StatusText.Text = ex.Message; }
    private void OnClosing(object? sender, CancelEventArgs e)
    {
        if (operations.Count == 0) return;
        e.Cancel = true;
        StatusText.Text = "작업이 진행 중입니다. 다운로드를 취소하거나 설치 작업이 끝난 뒤 닫아주세요.";
    }
}
