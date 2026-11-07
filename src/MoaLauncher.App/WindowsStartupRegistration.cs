using System.IO;
using Microsoft.Win32;

namespace MoaLauncher.App;

internal interface IStartupRegistration
{
    bool IsEnabled { get; }
    void SetEnabled(bool enabled);
}

internal sealed class WindowsStartupRegistration : IStartupRegistration
{
    internal const string BackgroundArgument = "--background";
    internal const string ValueName = "MoaLauncher";
    private const string RunKeyPath = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private readonly string registryPath;
    private readonly string executablePath;
    private readonly string command;

    internal WindowsStartupRegistration(string executablePath, string dataDirectory, string registryPath = RunKeyPath)
    {
        if (!Path.IsPathFullyQualified(executablePath) || executablePath.IndexOfAny(['"', '\r', '\n', '\0']) >= 0 ||
            !string.Equals(Path.GetExtension(executablePath), ".exe", StringComparison.OrdinalIgnoreCase))
            throw new ArgumentException("자동 실행에는 실행 파일의 전체 경로가 필요합니다.", nameof(executablePath));
        if (!Path.IsPathFullyQualified(dataDirectory) || dataDirectory.IndexOfAny(['"', '\r', '\n', '\0']) >= 0)
            throw new ArgumentException("자동 실행에는 데이터 폴더의 전체 경로가 필요합니다.", nameof(dataDirectory));
        this.registryPath = registryPath;
        this.executablePath = Path.GetFullPath(executablePath);
        command = $"{QuotePath(this.executablePath)} {BackgroundArgument} --data-dir {QuotePath(Path.GetFullPath(dataDirectory))}";
    }

    // Paths cannot contain quotes. Escape trailing backslashes before the closing quote.
    private static string QuotePath(string path) => $"\"{path}{new string('\\', path.Length - path.TrimEnd('\\').Length)}\"";

    public bool IsEnabled
    {
        get
        {
            using var key = Registry.CurrentUser.OpenSubKey(registryPath);
            return string.Equals(key?.GetValue(ValueName) as string, command, StringComparison.OrdinalIgnoreCase);
        }
    }

    public void SetEnabled(bool enabled)
    {
        if (enabled)
        {
            if (string.Equals(Path.GetFileName(executablePath), "dotnet.exe", StringComparison.OrdinalIgnoreCase))
                throw new InvalidOperationException("MoaLauncher.exe로 런처를 실행한 뒤 자동 실행을 켜 주세요.");
            // Windows limits Run commands to 260 characters, including the arguments.
            if (command.Length > 260)
                throw new InvalidOperationException("런처 경로가 너무 깁니다. 더 짧은 폴더로 옮긴 뒤 자동 실행을 켜 주세요.");
            using var key = Registry.CurrentUser.CreateSubKey(registryPath, writable: true);
            key.SetValue(ValueName, command, RegistryValueKind.String);
        }
        else
        {
            using var key = Registry.CurrentUser.OpenSubKey(registryPath, writable: true);
            key?.DeleteValue(ValueName, throwOnMissingValue: false);
        }
    }
}
