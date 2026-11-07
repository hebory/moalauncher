using System.IO;
using System.Runtime.InteropServices;
using Microsoft.Win32;
using MoaLauncher.App;

namespace MoaLauncher.App.SmokeTests;

internal static class StartupRegistrationChecks
{
    private const string TestKeyPrefix = @"Software\MoaLauncher\Tests\Startup-";

    internal static void Run()
    {
        if (!OperatingSystem.IsWindows()) throw new PlatformNotSupportedException("Startup registration checks require Windows.");

        var registryPath = TestKeyPrefix + Guid.NewGuid().ToString("N");
        try
        {
            var root = Path.GetPathRoot(Path.GetTempPath())
                ?? throw new InvalidOperationException("Could not determine the Windows volume root.");
            var executable = Path.GetFullPath(Path.Combine(Path.GetTempPath(), "Moa Launcher 한글", "MoaLauncher.exe"));
            var dataDirectory = Path.GetFullPath(Path.Combine(Path.GetTempPath(), "Moa Launcher 한글", "사용자 데이터"));
            var registration = new WindowsStartupRegistration(executable, dataDirectory, registryPath);

            Require(!registration.IsEnabled, "A missing startup value should be disabled by default.");
            registration.SetEnabled(true);
            Require(registration.IsEnabled, "Enabling startup should write the expected value.");
            Require(new WindowsStartupRegistration(executable, dataDirectory, registryPath).IsEnabled,
                "A newly constructed service should read the existing startup value.");

            var stored = ReadStartupValue(registryPath);
            Require(stored is not null, "The test registry value was not written.");
            var parsed = ParseCommandLine(stored!);
            Require(parsed.Length == 4, "The startup command should contain exactly four arguments.");
            Require(PathEquals(parsed[0], executable), "The executable path did not survive Windows command-line parsing.");
            Require(parsed[1] == WindowsStartupRegistration.BackgroundArgument, "The background argument is missing.");
            Require(parsed[2] == "--data-dir", "The data-directory argument is missing.");
            Require(PathEquals(parsed[3], dataDirectory), "The Korean path with spaces did not survive command-line parsing.");
            Require(stored == $"\"{executable}\" {WindowsStartupRegistration.BackgroundArgument} --data-dir \"{dataDirectory}\"",
                "The registered command does not match the expected startup command.");

            using (var key = Registry.CurrentUser.OpenSubKey(registryPath, writable: true)
                ?? throw new InvalidOperationException("Could not open the dedicated test registry key."))
            {
                key.SetValue("Unrelated", "preserve me", RegistryValueKind.String);
            }

            registration.SetEnabled(false);
            registration.SetEnabled(false);
            Require(!registration.IsEnabled, "Disabling startup twice should remain disabled.");
            using (var key = Registry.CurrentUser.OpenSubKey(registryPath)
                ?? throw new InvalidOperationException("Disabling startup unexpectedly removed the test key."))
            {
                Require(key.GetValue("Unrelated") as string == "preserve me", "Disabling startup removed an unrelated value.");
            }

            var rootRegistration = new WindowsStartupRegistration(executable, root, registryPath);
            rootRegistration.SetEnabled(true);
            var rootCommand = ReadStartupValue(registryPath)
                ?? throw new InvalidOperationException("The root-path startup value was not written.");
            var rootArguments = ParseCommandLine(rootCommand);
            Require(PathEquals(rootArguments[3], root), "A data directory ending in a backslash was not preserved.");
            rootRegistration.SetEnabled(false);

            registration.SetEnabled(true);
            stored = ReadStartupValue(registryPath)
                ?? throw new InvalidOperationException("The baseline startup value was not restored for failure checks.");
            var longDataDirectory = Path.Combine(Path.GetTempPath(), "MoaLauncher", new string('d', 240), "data");
            var longRegistration = new WindowsStartupRegistration(executable, longDataDirectory, registryPath);
            RequireThrows<InvalidOperationException>(() => longRegistration.SetEnabled(true),
                "A command longer than Windows' Run value limit should be rejected.");
            Require(ReadStartupValue(registryPath) == stored, "A rejected long command changed the existing startup value.");

            var dotnetHost = new WindowsStartupRegistration(
                Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "dotnet", "dotnet.exe"),
                dataDirectory, registryPath);
            RequireThrows<InvalidOperationException>(() => dotnetHost.SetEnabled(true),
                "Registering dotnet.exe as the startup executable should be rejected.");
            Require(ReadStartupValue(registryPath) == stored, "Rejecting dotnet.exe changed the existing startup value.");

            RequireThrows<ArgumentException>(() => new WindowsStartupRegistration("MoaLauncher.exe", dataDirectory, registryPath),
                "A relative executable path should be rejected.");
            RequireThrows<ArgumentException>(() => new WindowsStartupRegistration(executable, "relative-data", registryPath),
                "A relative data-directory path should be rejected.");
            RequireThrows<ArgumentException>(() => new WindowsStartupRegistration(
                    Path.ChangeExtension(executable, ".dll"), dataDirectory, registryPath),
                "A non-executable host path should be rejected.");
            Require(ReadStartupValue(registryPath) == stored, "Invalid path checks changed the existing startup value.");
        }
        finally
        {
            DeleteDedicatedTestKey(registryPath);
        }
    }

    private static string? ReadStartupValue(string registryPath)
    {
        using var key = Registry.CurrentUser.OpenSubKey(registryPath);
        return key?.GetValue(WindowsStartupRegistration.ValueName) as string;
    }

    private static string[] ParseCommandLine(string command)
    {
        var argv = CommandLineToArgvW(command, out var count);
        if (argv == IntPtr.Zero) throw new InvalidOperationException("CommandLineToArgvW could not parse the startup command.");
        try
        {
            var arguments = new string[count];
            for (var index = 0; index < count; index++)
            {
                var argument = Marshal.ReadIntPtr(argv, index * IntPtr.Size);
                arguments[index] = Marshal.PtrToStringUni(argument)
                    ?? throw new InvalidOperationException("CommandLineToArgvW returned a null argument.");
            }
            return arguments;
        }
        finally
        {
            _ = LocalFree(argv);
        }
    }

    private static void DeleteDedicatedTestKey(string registryPath)
    {
        if (!registryPath.StartsWith(TestKeyPrefix, StringComparison.Ordinal)) return;
        var suffix = registryPath[TestKeyPrefix.Length..];
        if (!Guid.TryParseExact(suffix, "N", out _)) return;
        Registry.CurrentUser.DeleteSubKeyTree(registryPath, throwOnMissingSubKey: false);
    }

    private static bool PathEquals(string left, string right) =>
        string.Equals(Path.GetFullPath(left).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar),
            Path.GetFullPath(right).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar),
            StringComparison.OrdinalIgnoreCase);

    private static void Require(bool condition, string message)
    {
        if (!condition) throw new InvalidOperationException(message);
    }

    private static void RequireThrows<TException>(Action action, string message) where TException : Exception
    {
        try
        {
            action();
        }
        catch (TException)
        {
            return;
        }
        throw new InvalidOperationException(message);
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CommandLineToArgvW(string commandLine, out int argumentCount);

    [DllImport("kernel32.dll")]
    private static extern IntPtr LocalFree(IntPtr memory);
}
