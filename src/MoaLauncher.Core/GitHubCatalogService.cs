using System.Net;
using System.Net.Http.Headers;
using System.Text.Json;

namespace MoaLauncher.Core;

/// <summary>Resolves repository visibility without placing credentials in shared app data.</summary>
public sealed class GitHubCatalogService
{
    private const string ApiRoot = "https://api.github.com/";
    private const int MaximumResponseBytes = 64 * 1024;
    private static readonly TimeSpan RequestTimeout = TimeSpan.FromSeconds(20);
    private readonly HttpClient client;
    private readonly IGitHubTokenProvider? tokenProvider;

    public GitHubCatalogService(HttpClient client, IGitHubTokenProvider? tokenProvider = null)
    {
        this.client = client ?? throw new ArgumentNullException(nameof(client));
        this.tokenProvider = tokenProvider;
    }

    /// <summary>Resolves whether a locally configured app's repository is private.</summary>
    public async Task<AppDefinition> ResolveVisibilityAsync(AppDefinition app, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(app);
        if (app.IsPrivate || string.IsNullOrWhiteSpace(app.Repository)) return app;
        if (!CatalogService.IsRepository(app.Repository)) throw new InvalidDataException("GitHub 저장소 주소 형식이 올바르지 않습니다.");

        var isPrivate = await ReadVisibilityAsync(app.Repository, GetToken(), missingMeansPrivate: false, cancellationToken)
            .ConfigureAwait(false);
        return isPrivate == true ? app with { IsPrivate = true } : app;
    }

    /// <summary>Builds a public-only export after anonymously verifying every included repository.</summary>
    public async Task<AppCatalog> CreatePublicExportAsync(IEnumerable<AppDefinition> apps,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(apps);
        var publicApps = new List<AppDefinition>();
        foreach (var app in apps)
        {
            cancellationToken.ThrowIfCancellationRequested();
            ArgumentNullException.ThrowIfNull(app);
            if (app.IsPrivate || string.IsNullOrWhiteSpace(app.Repository)) continue;
            if (!CatalogService.IsRepository(app.Repository)) throw new InvalidDataException("GitHub 저장소 주소 형식이 올바르지 않습니다.");

            var isPrivate = await ReadVisibilityAsync(app.Repository, token: null, missingMeansPrivate: true, cancellationToken)
                .ConfigureAwait(false);
            if (isPrivate == false) publicApps.Add(app);
        }

        var catalog = new AppCatalog { Apps = publicApps };
        CatalogService.Validate(catalog);
        return catalog;
    }

    private string? GetToken()
    {
        string? token;
        try { token = tokenProvider?.GetToken()?.Trim(); }
        catch (Exception ex) when (ex is not OperationCanceledException)
        {
            throw new InvalidOperationException("Windows 자격 증명 저장소에서 GitHub 인증 정보를 읽지 못했습니다.");
        }
        if (string.IsNullOrWhiteSpace(token)) return null;
        if (token.Length > 4096 || token.Any(character =>
                char.IsControl(character) || char.IsWhiteSpace(character) || character < 0x21 || character > 0x7E))
            throw new InvalidOperationException("비공개 저장소를 확인하려면 GitHub 인증을 설정하세요.");
        return token;
    }

    private async Task<bool?> ReadVisibilityAsync(string repository, string? token, bool missingMeansPrivate,
        CancellationToken cancellationToken)
    {
        var uri = new Uri(new Uri(ApiRoot), $"repos/{repository}");
        using var request = new HttpRequestMessage(HttpMethod.Get, uri);
        request.Headers.Accept.Add(new MediaTypeWithQualityHeaderValue("application/vnd.github+json"));
        request.Headers.UserAgent.ParseAdd($"MoaLauncher/{typeof(GitHubCatalogService).Assembly.GetName().Version?.ToString(3) ?? "0.0.0"}");
        request.Headers.TryAddWithoutValidation("X-GitHub-Api-Version", "2022-11-28");
        if (token is not null)
        {
            try { request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", token); }
            catch (Exception ex) when (ex is FormatException or ArgumentException)
            {
                throw new InvalidOperationException("비공개 저장소를 확인하려면 GitHub 인증을 설정하세요.");
            }
        }

        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeout.CancelAfter(RequestTimeout);
        try
        {
            using var response = await client.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, timeout.Token)
                .ConfigureAwait(false);
            if (response.RequestMessage?.RequestUri is not { } finalUri || finalUri != uri)
                throw new InvalidDataException("GitHub 저장소 정보 요청이 검사되지 않은 주소로 이동했습니다.");
            if (response.StatusCode == HttpStatusCode.NotFound)
            {
                if (missingMeansPrivate) return null;
                throw new InvalidOperationException("GitHub 저장소를 찾을 수 없거나 현재 계정에 접근 권한이 없습니다.");
            }
            if (response.StatusCode is HttpStatusCode.Unauthorized or HttpStatusCode.Forbidden)
                throw new InvalidOperationException("GitHub 인증 또는 저장소 읽기 권한을 확인하세요.");
            if (!response.IsSuccessStatusCode)
                throw new InvalidOperationException($"GitHub 저장소 정보 조회에 실패했습니다 (HTTP {(int)response.StatusCode}).");

            using var body = await ReadBoundedBodyAsync(response.Content, timeout.Token).ConfigureAwait(false);
            if (!body.RootElement.TryGetProperty("private", out var privateValue) ||
                privateValue.ValueKind is not (JsonValueKind.True or JsonValueKind.False))
                throw new InvalidDataException("GitHub 저장소 응답에 공개 여부가 없습니다.");
            return privateValue.GetBoolean();
        }
        catch (HttpRequestException)
        {
            throw new InvalidOperationException("GitHub 저장소 정보를 조회하지 못했습니다. 네트워크와 저장소 접근 권한을 확인하세요.");
        }
        catch (JsonException)
        {
            throw new InvalidDataException("GitHub 저장소 응답 형식이 올바르지 않습니다.");
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested && timeout.IsCancellationRequested)
        {
            throw new TimeoutException("GitHub 저장소 정보 조회 시간이 초과되었습니다.");
        }
    }

    private static async Task<JsonDocument> ReadBoundedBodyAsync(HttpContent content, CancellationToken cancellationToken)
    {
        if (content.Headers.ContentLength is > MaximumResponseBytes)
            throw new InvalidDataException("GitHub 저장소 응답이 허용 크기보다 큽니다.");

        await using var input = await content.ReadAsStreamAsync(cancellationToken).ConfigureAwait(false);
        using var output = new MemoryStream();
        var buffer = new byte[8192];
        while (true)
        {
            var count = await input.ReadAsync(buffer, cancellationToken).ConfigureAwait(false);
            if (count == 0) break;
            if (output.Length + count > MaximumResponseBytes)
                throw new InvalidDataException("GitHub 저장소 응답이 허용 크기보다 큽니다.");
            await output.WriteAsync(buffer.AsMemory(0, count), cancellationToken).ConfigureAwait(false);
        }

        return JsonDocument.Parse(output.GetBuffer().AsMemory(0, checked((int)output.Length)),
            new JsonDocumentOptions { MaxDepth = 16 });
    }
}
