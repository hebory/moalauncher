using System.IO;
using System.Net;
using System.Net.Http;
using System.Reflection;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Threading;
using MoaLauncher.App;
using MoaLauncher.Core;
using Button = System.Windows.Controls.Button;
using CheckBox = System.Windows.Controls.CheckBox;

namespace MoaLauncher.App.SmokeTests;

internal sealed class GitHubSettingsChecks : IDisposable
{
    private const string FirstToken = "github_pat_smoke_first_secret";
    private const string SecondToken = "github_pat_smoke_replacement_secret";
    private readonly HttpClient http = new(new AccountHandler());
    internal MemoryCredentials Credentials { get; } = new();
    internal GitHubAccountService AccountService => new(http);

    internal async Task RunAsync(MainWindow window, string dataDirectory)
    {
        var input = Field<PasswordBox>(window, "GitHubTokenInput");
        var connect = Field<Button>(window, "ConnectGitHubButton");
        var disconnect = Field<Button>(window, "DisconnectGitHubButton");
        var status = Field<TextBlock>(window, "GitHubStatusText");
        var items = Field<List<AppItem>>(window, "items");
        var definition = new AppDefinition
        {
            Id = "private-smoke", Name = "개인 앱", Repository = "smoke/private", IsPrivate = true,
            Packages = [new() { Kind = "zip", AssetPattern = "private.zip", ExecutablePath = "Private.exe" }]
        };
        var privateItem = new AppItem(definition) { Release = PrivateRelease(), Installed = new InstalledApp(
            definition.Id, "1.0.0", "Private.exe", "private", "zip", DateTimeOffset.UtcNow) };
        items.Add(privateItem);
        Invoke(window, "RefreshActionAvailability");
        try
        {
            Require(connect.IsEnabled && !disconnect.IsEnabled, "Initial GitHub buttons did not reflect disconnected state.");
            input.Password = FirstToken;
            connect.RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
            await DrainAsync(window);
            Require(Credentials.GetToken() == FirstToken && input.Password.Length == 0 && disconnect.IsEnabled,
                "Connecting did not save the token securely, clear the input, or enable disconnection.");
            Require(privateItem.Release is null && status.Text.Contains("smoke-user", StringComparison.Ordinal),
                "Connecting retained stale private release metadata or omitted the verified account.");

            input.Password = "invalid_smoke_secret";
            connect.RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
            await DrainAsync(window);
            Require(Credentials.GetToken() == FirstToken && input.Password.Length == 0 &&
                    !status.Text.Contains("invalid_smoke_secret", StringComparison.Ordinal),
                "A failed authentication replaced the saved token or exposed the entered secret.");

            var operations = Field<Dictionary<string, CancellationTokenSource>>(window, "operations");
            using (var operation = new CancellationTokenSource())
            {
                operations.Add("github-smoke-operation", operation);
                try
                {
                    Invoke(window, "RefreshActionAvailability");
                    Invoke(window, "DisconnectGitHub_Click", window, new RoutedEventArgs());
                    Require(!connect.IsEnabled && !disconnect.IsEnabled && Credentials.GetToken() == FirstToken,
                        "GitHub credentials changed during an installation.");
                }
                finally { operations.Remove("github-smoke-operation"); }
            }
            Invoke(window, "RefreshActionAvailability");
            privateItem.Release = PrivateRelease();
            input.Password = SecondToken;
            connect.RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
            await DrainAsync(window);
            Require(Credentials.GetToken() == SecondToken && privateItem.Release is null,
                "Replacing the credential did not invalidate the previous account's private release.");

            privateItem.Release = PrivateRelease();
            // The fake release service refuses this repository, as it would after access is revoked.
            await ((Task)(window.GetType().GetMethod("InstallItemAsync", BindingFlags.Instance | BindingFlags.NonPublic)!
                .Invoke(window, [privateItem, false]) ?? throw new InvalidOperationException("No install task returned.")));
            Require(privateItem.Release is null && privateItem.HasError,
                "An inaccessible private source retained an earlier release for installation.");
            privateItem.Release = PrivateRelease();
            disconnect.RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
            Require(Credentials.GetToken() is null && privateItem.Release is null && privateItem.IsInstalled &&
                    !disconnect.IsEnabled && items.Contains(privateItem),
                "Disconnect did not clear authorization, or removed the personal app/installation.");

            var editor = new AppEditorWindow(definition) { Owner = window };
            Require(Field<CheckBox>(editor, "PrivateRepositoryCheck").IsChecked == true,
                "The program editor lost the private flag.");
            _ = editor.Dispatcher.BeginInvoke(new Action(() => Invoke(editor, "Save_Click", editor, new RoutedEventArgs())));
            Require(editor.ShowDialog() == true && editor.Result?.IsPrivate == true,
                "Saving a private app in the editor lost its personal-only classification.");
            foreach (var path in Directory.EnumerateFiles(dataDirectory, "*", SearchOption.AllDirectories)
                         .Where(path => path.EndsWith(".json", StringComparison.OrdinalIgnoreCase) || path.EndsWith(".log", StringComparison.OrdinalIgnoreCase)))
            {
                var text = await File.ReadAllTextAsync(path);
                Require(!text.Contains(FirstToken, StringComparison.Ordinal) && !text.Contains(SecondToken, StringComparison.Ordinal) &&
                        !text.Contains("invalid_smoke_secret", StringComparison.Ordinal), "A token was written to a data file or log.");
            }
            Console.WriteLine("PASS: GitHub connect/replace/disconnect, busy guard, private metadata invalidation, editor privacy, and no token in data/logs.");
        }
        finally
        {
            items.Remove(privateItem);
            Credentials.DeleteToken();
            Invoke(window, "ApplyFilter");
        }
    }

    private static AppRelease PrivateRelease() => new("v2.0.0", "Private", "Private notes",
        new Uri("https://github.com/smoke/private/releases/tag/v2.0.0"), DateTimeOffset.UtcNow, [], IsPrivate: true);
    private static async Task DrainAsync(MainWindow window)
    {
        var deadline = DateTime.UtcNow.AddSeconds(5);
        while (Field<bool>(window, "catalogChanging"))
        {
            if (DateTime.UtcNow >= deadline) throw new TimeoutException("GitHub settings operation did not complete.");
            await window.Dispatcher.InvokeAsync(() => { }, DispatcherPriority.ApplicationIdle);
            await Task.Delay(10);
        }
    }
    private static T Field<T>(object instance, string name) => (T)(instance.GetType()
        .GetField(name, BindingFlags.Instance | BindingFlags.NonPublic)?.GetValue(instance)
        ?? throw new MissingFieldException(name));
    private static void Invoke(object instance, string name, params object[] args) =>
        (instance.GetType().GetMethod(name, BindingFlags.Instance | BindingFlags.NonPublic)
        ?? throw new MissingMethodException(name)).Invoke(instance, args);
    private static void Require(bool value, string message) { if (!value) throw new InvalidOperationException(message); }
    public void Dispose() => http.Dispose();

    internal sealed class MemoryCredentials : IGitHubCredentialStore
    {
        private string? token;
        public string? GetToken() => token;
        public void SaveToken(string value) => token = value;
        public void DeleteToken() => token = null;
    }
    private sealed class AccountHandler : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            var token = request.Headers.Authorization?.Parameter;
            var valid = token is FirstToken or SecondToken;
            return Task.FromResult(new HttpResponseMessage(valid ? HttpStatusCode.OK : HttpStatusCode.Unauthorized)
            {
                RequestMessage = request,
                Content = new StringContent(valid ? "{\"login\":\"smoke-user\"}" : "invalid_smoke_secret")
            });
        }
    }
}
