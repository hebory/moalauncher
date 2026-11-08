using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Threading;
using System.Windows.Forms;
using MoaLauncher.App;
using MoaLauncher.Core;
using System.Net.Http;
using System.Text.Json;

namespace MoaLauncher.App.SmokeTests;

internal static class Program
{
    private const string InitialCheckedText = "아직 확인하지 않음";
    private static readonly BindingFlags PrivateInstance = BindingFlags.Instance | BindingFlags.NonPublic;

    [STAThread]
    private static int Main(string[] args)
    {
        if (args.Length == 2 && args[0] == "--verify-published-self-update")
            return VerifyPublishedSelfUpdateAsync(args[1]).GetAwaiter().GetResult();
        if (args.Contains("--activation-probe", StringComparer.OrdinalIgnoreCase))
            return RunActivationProbe();

        var testDataRoot = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "testdata"));
        var dataDirectory = Path.GetFullPath(Path.Combine(testDataRoot, "tray-smoke-" + Guid.NewGuid().ToString("N")));
        EnsureUnderTestData(testDataRoot, dataDirectory);
        Directory.CreateDirectory(dataDirectory);
        WriteTestData(dataDirectory);

        var startsInBackground = args.Contains(WindowsStartupRegistration.BackgroundArgument, StringComparer.OrdinalIgnoreCase);
        var app = new App();
        var startupRegistration = new FakeStartupRegistration();
        var updateChecks = new UpdateNotificationChecks();
        app.StartupRegistration = startupRegistration;
        app.MainWindowFactory = () => new MainWindow(updateChecks.ReleaseService, updateChecks.InstallationService);
        SetDataDirectory(dataDirectory);
        app.InitializeComponent();
        app.Startup += (_, _) => app.Dispatcher.BeginInvoke(new Action(async () =>
        {
            try
            {
                await RunLifecycleChecksAsync(app, dataDirectory, testDataRoot, startupRegistration, updateChecks, startsInBackground);
            }
            catch (Exception ex)
            {
                Environment.ExitCode = 1;
                Console.Error.WriteLine(ex);
                ForceShutdown(app);
            }
        }));

        var result = app.Run();
        try
        {
            if (result != 0) throw new InvalidOperationException($"Application.Run returned {result}.");
            if (!app.IsExiting) throw new InvalidOperationException("Application.Run ended before an explicit exit request.");
            AssertTrayWasDisposed(app);
            Console.WriteLine($"PASS: {(startsInBackground ? "background" : "normal")} startup, tray lifecycle, activation, startup setting, and cleanup.");
        }
        catch (Exception ex)
        {
            Environment.ExitCode = 1;
            Console.Error.WriteLine(ex);
        }
        finally
        {
            if (!app.IsExiting)
            {
                try { app.RequestExit(); }
                catch { }
            }
            EnsureUnderTestData(testDataRoot, dataDirectory);
            if (Directory.Exists(dataDirectory)) Directory.Delete(dataDirectory, recursive: true);
        }
        return Environment.ExitCode;
    }

    private static async Task<int> VerifyPublishedSelfUpdateAsync(string directory)
    {
        try
        {
            directory = Path.GetFullPath(directory);
            using var http = new HttpClient(new HttpClientHandler { AllowAutoRedirect = false }) { Timeout = TimeSpan.FromMinutes(15) };
            var releases = new GitHubReleaseService(http, Path.Combine(directory, "cache"));
            var release = await releases.GetLatestAsync(SelfUpdateService.Definition);
            if (release.IsCached) throw new InvalidOperationException("A live release response is required.");
            var prepared = await new SelfUpdateService(http, directory).PrepareAsync(release.Release, "0.1.0");
            var json = JsonSerializer.Serialize(prepared, new JsonSerializerOptions { WriteIndented = true });
            await File.WriteAllTextAsync(Path.Combine(directory, "verified-installer.json"), json);
            Console.WriteLine(json);
            return 0;
        }
        catch (Exception ex) { Console.Error.WriteLine(ex.Message); return 1; }
    }

    private static int RunActivationProbe()
    {
        var app = new App();
        app.InitializeComponent();
        return app.Run();
    }

    private static async Task RunLifecycleChecksAsync(App app, string dataDirectory, string testDataRoot,
        FakeStartupRegistration startupRegistration, UpdateNotificationChecks updateChecks, bool startsInBackground)
    {
        await WaitUntilAsync(() => app.MainWindow is not null && GetField<bool>(app.MainWindow, "ready"),
            "MainWindow initialization", app.Dispatcher);
        var window = (MainWindow)app.MainWindow;
        var timer = GetField<DispatcherTimer>(window, "timer");
        if (!timer.IsEnabled) throw new InvalidOperationException("Periodic timer was not enabled after initialization.");
        var tray = GetField<object>(app, "trayIcon");
        var notifyIcon = GetField<NotifyIcon>(tray, "notifyIcon");
        var menu = GetField<ContextMenuStrip>(tray, "contextMenu");
        if (!notifyIcon.Visible) throw new InvalidOperationException("Tray icon is not visible.");
        if (menu.Items.Count < 3 || menu.Items[0].Text != "모아 런처 열기" || menu.Items[2].Text != "종료")
            throw new InvalidOperationException("Tray menu is missing its open or exit action.");

        if (startsInBackground)
        {
            if (window.IsLoaded || window.IsVisible)
                throw new InvalidOperationException("Background startup loaded or showed the main window.");
        }
        else if (!window.IsLoaded || !window.IsVisible)
        {
            throw new InvalidOperationException("Normal startup did not show the main window.");
        }

        StartupRegistrationChecks.Run();
        await ArtworkChecks.RunAsync(dataDirectory);
        await CheckStartupSettingToggleAsync(window, startupRegistration);

        if (!startsInBackground)
        {
            window.Close();
            if (window.IsVisible || !window.IsLoaded || !timer.IsEnabled)
                throw new InvalidOperationException("Close did not hide the main window while preserving its timer and lifetime.");
        }

        var checkedLabel = GetField<TextBlock>(window, "LastCheckedText");
        timer.Stop();
        timer.Interval = TimeSpan.FromMilliseconds(100);
        timer.Start();
        await WaitUntilAsync(() => checkedLabel.Text != InitialCheckedText, "hidden timer update", app.Dispatcher);
        if (window.IsVisible) throw new InvalidOperationException("A hidden periodic check unexpectedly showed the window.");
        timer.Stop();

        app.RestoreMainWindow();
        await WaitUntilAsync(() => window.IsLoaded, "first window load after background startup", app.Dispatcher);
        if (!window.IsVisible || !window.IsLoaded) throw new InvalidOperationException("RestoreMainWindow did not show the window.");
        window.WindowState = WindowState.Minimized;
        app.HideMainWindow();
        app.RestoreMainWindow();
        if (!window.IsVisible || window.WindowState != WindowState.Normal)
            throw new InvalidOperationException("RestoreMainWindow did not restore a minimized window.");

        app.HideMainWindow();
        var dataPath = Path.GetFullPath(dataDirectory).ToUpperInvariant();
        var identity = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(dataPath))).Substring(0, 16);
        var activationName = @"Local\MoaLauncher-Activate-" + identity;
        using (var activationClient = EventWaitHandle.OpenExisting(activationName)) activationClient.Set();
        await WaitUntilAsync(() => window.IsVisible, "named activation event restore", app.Dispatcher);

        await CheckDuplicateActivationAsync(app, window, testDataRoot, dataDirectory);

        app.HideMainWindow();
        menu.Items[0].PerformClick();
        if (!window.IsVisible) throw new InvalidOperationException("Tray open menu action did not restore the window.");

        await updateChecks.RunAsync(app, window);

        var operations = GetField<Dictionary<string, CancellationTokenSource>>(window, "operations");
        using (var syntheticOperation = new CancellationTokenSource())
        {
            operations.Add("smoke-test-active-operation", syntheticOperation);
            try
            {
                app.RequestExit();
                if (app.IsExiting || !window.IsVisible)
                    throw new InvalidOperationException("Exit request was not refused while a synthetic operation was active.");
            }
            finally
            {
                operations.Remove("smoke-test-active-operation");
            }
        }

        app.HideMainWindow();
        menu.Items[2].PerformClick();
        if (!app.IsExiting) throw new InvalidOperationException("Tray exit menu action did not request application exit.");
    }

    private static async Task CheckStartupSettingToggleAsync(MainWindow window, FakeStartupRegistration registration)
    {
        var checkBox = GetField<System.Windows.Controls.CheckBox>(window, "AutoStartCheck");
        if (!checkBox.IsEnabled || checkBox.IsChecked != registration.IsEnabled)
            throw new InvalidOperationException("Startup checkbox did not reflect the injected registration state.");

        checkBox.IsChecked = true;
        if (!registration.IsEnabled || checkBox.IsChecked != true)
            throw new InvalidOperationException("Enabling startup registration did not update the fake registration.");

        registration.FailNextSet = true;
        checkBox.IsChecked = false;
        if (!registration.IsEnabled || checkBox.IsChecked != true)
            throw new InvalidOperationException("A failed startup registration change did not restore the actual state.");

        checkBox.IsChecked = false;
        if (registration.IsEnabled || checkBox.IsChecked != false)
            throw new InvalidOperationException("Disabling startup registration did not update the fake registration.");
        await window.Dispatcher.InvokeAsync(() => { }, DispatcherPriority.ApplicationIdle);
    }

    private static async Task CheckDuplicateActivationAsync(App app, MainWindow window, string testDataRoot, string dataDirectory)
    {
        app.HideMainWindow();
        using (var backgroundProbe = StartActivationProbe(testDataRoot, dataDirectory, background: true))
        {
            await WaitForProbeExitAsync(backgroundProbe, "background duplicate activation");
            await Task.Delay(150);
            if (window.IsVisible) throw new InvalidOperationException("A background duplicate unexpectedly restored the hidden window.");
        }

        using (var interactiveProbe = StartActivationProbe(testDataRoot, dataDirectory, background: false))
        {
            await WaitForProbeExitAsync(interactiveProbe, "interactive duplicate activation");
        }
        await WaitUntilAsync(() => window.IsVisible, "interactive duplicate restoring the window", app.Dispatcher);
    }

    private static Process StartActivationProbe(string testDataRoot, string dataDirectory, bool background)
    {
        EnsureUnderTestData(testDataRoot, dataDirectory);
        var executable = Environment.ProcessPath ?? throw new InvalidOperationException("Could not determine the smoke test process path.");
        var startInfo = new ProcessStartInfo(executable)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            WindowStyle = ProcessWindowStyle.Hidden,
            WorkingDirectory = AppContext.BaseDirectory
        };
        if (string.Equals(Path.GetFileNameWithoutExtension(executable), "dotnet", StringComparison.OrdinalIgnoreCase))
            startInfo.ArgumentList.Add(Assembly.GetExecutingAssembly().Location);
        startInfo.ArgumentList.Add("--activation-probe");
        if (background) startInfo.ArgumentList.Add(WindowsStartupRegistration.BackgroundArgument);
        startInfo.ArgumentList.Add("--data-dir");
        startInfo.ArgumentList.Add(dataDirectory);
        return Process.Start(startInfo) ?? throw new InvalidOperationException("Could not start an activation probe process.");
    }

    private static async Task WaitForProbeExitAsync(Process process, string description)
    {
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(10));
        try { await process.WaitForExitAsync(timeout.Token); }
        catch (OperationCanceledException)
        {
            try { process.Kill(entireProcessTree: true); }
            catch (InvalidOperationException) { }
            catch (System.ComponentModel.Win32Exception) { }
            throw new TimeoutException($"Timed out waiting for {description}.");
        }
        if (process.ExitCode != 0) throw new InvalidOperationException($"{description} exited with code {process.ExitCode}.");
    }

    private static async Task WaitUntilAsync(Func<bool> condition, string description, Dispatcher dispatcher)
    {
        var stopwatch = Stopwatch.StartNew();
        while (!condition())
        {
            if (stopwatch.Elapsed > TimeSpan.FromSeconds(10))
                throw new TimeoutException($"Timed out waiting for {description}.");
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

    private static void SetDataDirectory(string path)
    {
        var property = typeof(App).GetProperty(nameof(App.DataDirectory), BindingFlags.Static | BindingFlags.Public)
            ?? throw new MissingMemberException(typeof(App).FullName, nameof(App.DataDirectory));
        var setter = property.GetSetMethod(nonPublic: true)
            ?? throw new MissingMethodException(typeof(App).FullName, "set_DataDirectory");
        setter.Invoke(null, [path]);
    }

    private static void ForceShutdown(App app)
    {
        var property = typeof(App).GetProperty(nameof(App.IsExiting), BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new MissingMemberException(typeof(App).FullName, nameof(App.IsExiting));
        property.GetSetMethod(nonPublic: true)!.Invoke(app, [true]);
        app.Shutdown(1);
    }

    private static void EnsureUnderTestData(string testDataRoot, string targetPath)
    {
        var root = Path.GetFullPath(testDataRoot).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
        var target = Path.GetFullPath(targetPath);
        if (!target.StartsWith(root + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException("Smoke test data path escaped its dedicated testdata folder.");
    }

    private static void AssertTrayWasDisposed(App app)
    {
        var tray = GetField<object>(app, "trayIcon");
        if (!GetField<bool>(tray, "disposed")) throw new InvalidOperationException("Tray service was not disposed on app exit.");
        var notifyIcon = GetField<NotifyIcon>(tray, "notifyIcon");
        if (notifyIcon.Visible) throw new InvalidOperationException("NotifyIcon remained visible after app exit.");
    }

    private static void WriteTestData(string dataDirectory)
    {
        File.WriteAllText(Path.Combine(dataDirectory, "catalog.json"), """
            {
              "schemaVersion": 1,
              "apps": [{
                "id": "moaview",
                "name": "모아뷰",
                "description": "스모크 테스트용 로컬 항목",
                "category": "테스트",
                "accentColor": "#8BA3FF",
                "monogram": "Mv",
                "repository": "",
                "packages": [
                  { "architecture": "x64", "kind": "zip", "assetPattern": "MoaView-*-x64.zip", "executablePath": "MoaView.exe" },
                  { "architecture": "x86", "kind": "zip", "assetPattern": "MoaView-*-x86.zip", "executablePath": "MoaView.exe" },
                  { "architecture": "arm64", "kind": "zip", "assetPattern": "MoaView-*-arm64.zip", "executablePath": "MoaView.exe" }
                ]
              }]
            }
            """);

        File.WriteAllText(Path.Combine(dataDirectory, "settings.json"), """
            { "checkOnStartup": false, "periodicChecks": true, "checkIntervalMinutes": 60 }
            """);
    }

    private sealed class FakeStartupRegistration : IStartupRegistration
    {
        public bool IsEnabled { get; private set; }
        internal bool FailNextSet { get; set; }

        public void SetEnabled(bool enabled)
        {
            if (FailNextSet)
            {
                FailNextSet = false;
                throw new InvalidOperationException("smoke test startup registration failure");
            }
            IsEnabled = enabled;
        }
    }
}
