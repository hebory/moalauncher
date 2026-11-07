using System.IO;
using System.Windows;
using System.Windows.Interop;
using System.Runtime.InteropServices;

namespace MoaLauncher.App;

public partial class App : Application
{
    private Mutex? instanceMutex;
    private bool ownsMutex;
    private EventWaitHandle? activationEvent;
    private RegisteredWaitHandle? activationRegistration;
    private TrayIconService? trayIcon;
    private IStartupRegistration? startupRegistration;
    internal IStartupRegistration StartupRegistration
    {
        get => startupRegistration ??= new WindowsStartupRegistration(
            Environment.ProcessPath ?? throw new InvalidOperationException("런처 실행 파일의 위치를 찾지 못했습니다."), DataDirectory);
        set => startupRegistration = value;
    }
    internal bool IsExiting { get; private set; }
    public static string DataDirectory { get; private set; } = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "MoaLauncher");

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);
        var startInBackground = e.Args.Contains(WindowsStartupRegistration.BackgroundArgument, StringComparer.OrdinalIgnoreCase);
        var index = Array.IndexOf(e.Args, "--data-dir");
        if (index >= 0 && index + 1 < e.Args.Length) DataDirectory = Path.GetFullPath(e.Args[index + 1]);
        var identity = Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(DataDirectory.ToUpperInvariant())))[..16];
        // Create the event before the mutex so a second launch cannot lose an early activation request.
        activationEvent = new EventWaitHandle(false, EventResetMode.AutoReset, @"Local\MoaLauncher-Activate-" + identity);
        instanceMutex = new Mutex(true, @"Local\MoaLauncher-" + identity, out ownsMutex);
        if (!ownsMutex)
        {
            if (!startInBackground) activationEvent.Set();
            Shutdown();
            return;
        }
        Directory.CreateDirectory(DataDirectory);
        EventManager.RegisterClassHandler(typeof(Window), FrameworkElement.LoadedEvent, new RoutedEventHandler((sender, _) =>
        {
            if (sender is not Window window) return;
            var enabled = 0;
            DwmSetWindowAttribute(new WindowInteropHelper(window).Handle, 20, ref enabled, sizeof(int));
        }));
        DispatcherUnhandledException += (_, args) =>
        {
            Log(args.Exception);
            if (MainWindow?.IsVisible == true)
                MessageBox.Show("작업을 완료하지 못했습니다.\n" + args.Exception.Message, "Moa Launcher", MessageBoxButton.OK, MessageBoxImage.Warning);
            args.Handled = true;
        };
        try
        {
            MainWindow = new MainWindow();
            trayIcon = new TrayIconService(RestoreMainWindow, RequestExit);
            activationRegistration = ThreadPool.RegisterWaitForSingleObject(activationEvent, (_, _) =>
            {
                if (!Dispatcher.HasShutdownStarted)
                    Dispatcher.BeginInvoke(new Action(RestoreMainWindow));
            }, null, Timeout.Infinite, executeOnlyOnce: false);
            if (startInBackground)
                _ = ((MainWindow)MainWindow).EnsureInitializedAsync();
            else
                MainWindow.Show();
        }
        catch (Exception ex)
        {
            Log(ex);
            if (!startInBackground)
                MessageBox.Show("런처를 시작하지 못했습니다.\n" + ex.Message, "Moa Launcher", MessageBoxButton.OK, MessageBoxImage.Error);
            IsExiting = true;
            Shutdown(1);
        }
    }

    internal void HideMainWindow()
    {
        if (IsExiting) return;
        MainWindow?.Hide();
        trayIcon?.NotifyHidden();
    }

    internal void RestoreMainWindow()
    {
        if (IsExiting || MainWindow is null) return;
        MainWindow.Show();
        if (MainWindow is MainWindow window) _ = window.EnsureInitializedAsync();
        if (MainWindow.WindowState == WindowState.Minimized) MainWindow.WindowState = WindowState.Normal;
        // A modal editor/detail window should stay above its owner when reopening from the tray.
        var active = MainWindow.OwnedWindows.Cast<Window>().LastOrDefault(window => window.IsVisible) ?? MainWindow;
        active.Activate();
    }

    internal void RequestExit()
    {
        if (IsExiting) return;
        if (MainWindow is MainWindow window && !window.TryPrepareExit())
        {
            RestoreMainWindow();
            return;
        }
        IsExiting = true;
        Shutdown();
    }

    protected override void OnSessionEnding(SessionEndingCancelEventArgs e)
    {
        // Windows logoff/shutdown must not turn into another close-to-tray operation.
        IsExiting = true;
        base.OnSessionEnding(e);
        if (e.Cancel) IsExiting = false;
    }

    [DllImport("dwmapi.dll")]
    private static extern int DwmSetWindowAttribute(IntPtr handle, int attribute, ref int value, int size);

    internal static void Log(Exception error)
    {
        try
        {
            var path = Path.Combine(DataDirectory, "launcher.log");
            if (File.Exists(path) && new FileInfo(path).Length > 2_000_000) File.Move(path, path + ".old", true);
            File.AppendAllText(path, $"{DateTimeOffset.Now:O} {error}\n");
        }
        catch (IOException) { }
        catch (UnauthorizedAccessException) { }
    }

    protected override void OnExit(ExitEventArgs e)
    {
        IsExiting = true;
        activationRegistration?.Unregister(null);
        trayIcon?.Dispose();
        activationEvent?.Dispose();
        if (ownsMutex) instanceMutex?.ReleaseMutex();
        instanceMutex?.Dispose();
        base.OnExit(e);
    }
}
