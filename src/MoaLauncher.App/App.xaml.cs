using System.IO;
using System.Windows;
using System.Windows.Interop;
using System.Runtime.InteropServices;

namespace MoaLauncher.App;

public partial class App : Application
{
    private Mutex? instanceMutex;
    private bool ownsMutex;
    public static string DataDirectory { get; private set; } = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "MoaLauncher");

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);
        var index = Array.IndexOf(e.Args, "--data-dir");
        if (index >= 0 && index + 1 < e.Args.Length) DataDirectory = Path.GetFullPath(e.Args[index + 1]);
        var identity = Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(DataDirectory.ToUpperInvariant())))[..16];
        instanceMutex = new Mutex(true, @"Local\MoaLauncher-" + identity, out ownsMutex);
        if (!ownsMutex)
        {
            MessageBox.Show("모아 런처가 이미 실행 중입니다. 작업 표시줄에서 열어주세요.", "Moa Launcher");
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
            MessageBox.Show("작업을 완료하지 못했습니다.\n" + args.Exception.Message, "Moa Launcher", MessageBoxButton.OK, MessageBoxImage.Warning);
            args.Handled = true;
        };
        MainWindow = new MainWindow();
        MainWindow.Show();
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
        if (ownsMutex) instanceMutex?.ReleaseMutex();
        instanceMutex?.Dispose();
        base.OnExit(e);
    }
}
