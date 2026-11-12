using System.Windows;
using MoaLauncher.Core;

namespace MoaLauncher.App;

public partial class MainWindow
{
    private readonly IGitHubCredentialStore gitHubCredentials;
    private readonly GitHubAccountService gitHubAccount;
    private readonly GitHubCatalogService gitHubCatalog;
    private bool gitHubConnected;

    private bool CanChangeGitHubConnection => ready && !batchUpdating && !catalogChanging && !checking &&
        operations.Count == 0 && !items.Any(item => item.Busy) &&
        !OwnedWindows.OfType<AppEditorWindow>().Any(window => window.IsVisible);

    private void ReadGitHubConnection()
    {
        try
        {
            gitHubConnected = !string.IsNullOrEmpty(gitHubCredentials.GetToken());
            GitHubStatusText.Text = gitHubConnected ? "GitHub 토큰이 연결되어 있습니다." : "GitHub에 연결하지 않았습니다.";
        }
        catch
        {
            gitHubConnected = false;
            GitHubStatusText.Text = "Windows에 저장한 GitHub 인증 정보를 읽지 못했습니다.";
        }
    }

    private async void ConnectGitHub_Click(object sender, RoutedEventArgs e)
    {
        if (!CanChangeGitHubConnection) return;
        var token = GitHubTokenInput.Password;
        GitHubTokenInput.Clear();
        if (string.IsNullOrWhiteSpace(token))
        {
            GitHubStatusText.Text = "GitHub 개인 액세스 토큰을 입력하세요.";
            return;
        }
        catalogChanging = true;
        RefreshActionAvailability();
        try
        {
            var login = await gitHubAccount.VerifyTokenAsync(token, lifetime.Token);
            gitHubCredentials.SaveToken(token);
            gitHubConnected = true;
            ClearPrivateReleaseState();
            GitHubStatusText.Text = $"GitHub @{login} 연결 완료 · ‘업데이트 확인’으로 저장소 접근 권한을 확인하세요.";
            StatusText.Text = "GitHub 인증 정보를 안전하게 저장했습니다.";
        }
        catch (OperationCanceledException) when (lifetime.IsCancellationRequested) { }
        catch
        {
            // Do not log credential-bearing exceptions, response bodies, or the entered secret.
            GitHubStatusText.Text = "GitHub 연결을 확인하거나 저장하지 못했습니다. 토큰·만료일·네트워크를 확인하세요.";
        }
        finally
        {
            token = string.Empty;
            catalogChanging = false;
            RefreshActionAvailability();
        }
    }

    private void DisconnectGitHub_Click(object sender, RoutedEventArgs e)
    {
        if (!CanChangeGitHubConnection) return;
        GitHubTokenInput.Clear();
        try
        {
            gitHubCredentials.DeleteToken();
            gitHubConnected = false;
            ClearPrivateReleaseState();
            GitHubStatusText.Text = "GitHub 연결을 해제했습니다.";
            StatusText.Text = "저장된 토큰을 제거했습니다. 개인 카탈로그와 설치된 앱은 유지됩니다.";
        }
        catch { GitHubStatusText.Text = "Windows에 저장한 GitHub 인증 정보를 제거하지 못했습니다."; }
        finally { RefreshActionAvailability(); }
    }

    private void ClearPrivateReleaseState()
    {
        foreach (var item in items.Where(item => item.IsPrivate))
        {
            item.Release = null;
            item.IsCached = false;
            item.Error = null;
            item.ProgressText = "";
            item.Refresh();
        }
        ApplyFilter();
    }

    private void OpenGitHubTokenSettings_Click(object sender, RoutedEventArgs e) =>
        OpenPath("https://github.com/settings/personal-access-tokens/new");
}
