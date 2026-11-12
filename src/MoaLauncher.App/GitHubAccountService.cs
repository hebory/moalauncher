using System.IO;
using System.Net;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Text.RegularExpressions;
using System.Text.Json;

namespace MoaLauncher.App;

/// <summary>Validates a GitHub personal access token without logging or caching it.</summary>
internal sealed class GitHubAccountService(HttpClient http)
{
    private static readonly Uri UserEndpoint = new("https://api.github.com/user", UriKind.Absolute);
    private static readonly Regex GitHubLoginPattern = new("^[A-Za-z0-9](?:[A-Za-z0-9]|-(?=[A-Za-z0-9])){0,38}$",
        RegexOptions.CultureInvariant | RegexOptions.Compiled);
    private const int MaximumResponseBytes = 64 * 1024;
    private static readonly TimeSpan RequestTimeout = TimeSpan.FromSeconds(20);
    private readonly HttpClient client = http ?? throw new ArgumentNullException(nameof(http));

    internal async Task<string> VerifyTokenAsync(string token, CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(token) || token.Any(character => char.IsWhiteSpace(character) || char.IsControl(character)))
            throw new InvalidOperationException("GitHub 인증 정보를 확인하세요. 공백과 줄바꿈은 허용되지 않습니다.");

        using var request = new HttpRequestMessage(HttpMethod.Get, UserEndpoint);
        request.Headers.Accept.Add(new MediaTypeWithQualityHeaderValue("application/vnd.github+json"));
        request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", token);
        request.Headers.UserAgent.ParseAdd("MoaLauncher/GitHubAuthentication");
        request.Headers.TryAddWithoutValidation("X-GitHub-Api-Version", "2022-11-28");

        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeout.CancelAfter(RequestTimeout);
        try
        {
            using var response = await client.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, timeout.Token)
                .ConfigureAwait(false);
            if (response.RequestMessage?.RequestUri != UserEndpoint || (int)response.StatusCode is >= 300 and <= 399)
                throw new InvalidOperationException("GitHub 인증 요청의 주소가 변경되어 인증을 중단했습니다.");
            if (response.StatusCode == HttpStatusCode.Unauthorized)
                throw new InvalidOperationException("GitHub 인증에 실패했습니다. 인증 정보를 확인하세요.");
            if (response.StatusCode == HttpStatusCode.Forbidden)
                throw new InvalidOperationException("GitHub에서 인증 요청을 제한했습니다. 잠시 뒤 다시 시도하세요.");
            if (!response.IsSuccessStatusCode)
                throw new InvalidOperationException("GitHub 인증에 실패했습니다. 요청을 완료하지 못했습니다.");
            return await ReadLoginAsync(response.Content, timeout.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            throw;
        }
        catch (OperationCanceledException) when (timeout.IsCancellationRequested)
        {
            throw new TimeoutException("GitHub 인증 확인 시간이 초과되었습니다.");
        }
        catch (HttpRequestException)
        {
            throw new InvalidOperationException("GitHub에 연결하지 못했습니다. 인터넷 연결을 확인하세요.");
        }
    }

    private static async Task<string> ReadLoginAsync(HttpContent content, CancellationToken cancellationToken)
    {
        if (content.Headers.ContentLength is > MaximumResponseBytes)
            throw new InvalidOperationException("GitHub 계정 응답이 허용된 크기를 초과합니다.");

        try
        {
            await using var stream = await content.ReadAsStreamAsync(cancellationToken).ConfigureAwait(false);
            using var body = new MemoryStream();
            var buffer = new byte[4096];
            while (true)
            {
                var count = await stream.ReadAsync(buffer, cancellationToken).ConfigureAwait(false);
                if (count == 0) break;
                if (body.Length + count > MaximumResponseBytes)
                    throw new InvalidOperationException("GitHub 계정 응답이 허용된 크기를 초과합니다.");
                body.Write(buffer, 0, count);
            }

            using var json = JsonDocument.Parse(body.GetBuffer().AsMemory(0, checked((int)body.Length)));
            if (!json.RootElement.TryGetProperty("login", out var login) || login.ValueKind != JsonValueKind.String ||
                login.GetString() is not { } loginName || !GitHubLoginPattern.IsMatch(loginName))
                throw new InvalidOperationException("GitHub 계정 응답에 유효한 사용자 이름이 없습니다.");
            return loginName;
        }
        catch (JsonException)
        {
            throw new InvalidOperationException("GitHub 계정 응답을 읽지 못했습니다.");
        }
        catch (IOException)
        {
            throw new InvalidOperationException("GitHub 계정 응답을 받지 못했습니다.");
        }
    }
}
