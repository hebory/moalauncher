using System.IO;
using System.Net;
using System.Net.Http;
using MoaLauncher.App;

namespace MoaLauncher.App.SmokeTests;

internal static class GitHubCredentialChecks
{
    internal static async Task RunAsync(string testDataDirectory)
    {
        // Unique targets under the test data identity never read or modify the user's connection.
        var identity = Path.Combine(testDataDirectory, "github-credentials-" + Guid.NewGuid().ToString("N"));
        var store = new WindowsGitHubCredentialStore(identity);
        var sameStore = new WindowsGitHubCredentialStore(identity.ToUpperInvariant() + Path.DirectorySeparatorChar);
        var otherStore = new WindowsGitHubCredentialStore(identity + "-other");
        const string first = "github_pat_native_smoke_first";
        const string second = "github_pat_native_smoke_second";
        try
        {
            Require(store.GetToken() is null && otherStore.GetToken() is null, "A unique test credential target was not empty.");
            store.SaveToken(first);
            Require(sameStore.GetToken() == first && otherStore.GetToken() is null,
                "Credential persistence or data-directory isolation failed.");
            sameStore.SaveToken(second);
            Require(store.GetToken() == second, "Replacing a saved credential failed.");
            try { store.SaveToken("invalid\nsecret"); throw new InvalidOperationException("Invalid credentials were accepted."); }
            catch (ArgumentException) { }
            Require(store.GetToken() == second, "Rejecting a malformed token changed the saved credential.");
            store.DeleteToken();
            store.DeleteToken();
            Require(sameStore.GetToken() is null, "Deleting a credential was not persistent and idempotent.");
        }
        finally
        {
            store.DeleteToken();
            otherStore.DeleteToken();
        }

        using (var client = new HttpClient(new ResponseHandler(request =>
        {
            Require(request.RequestUri == new Uri("https://api.github.com/user"), "Authentication left the GitHub API endpoint.");
            Require(request.Headers.Authorization?.Scheme == "Bearer" && request.Headers.Authorization.Parameter == first,
                "Authentication did not attach the supplied credential to the API request.");
            return new HttpResponseMessage(HttpStatusCode.OK) { RequestMessage = request, Content = new StringContent("{\"login\":\"smoke-user\"}") };
        })))
        {
            Require(await new GitHubAccountService(client).VerifyTokenAsync(first) == "smoke-user", "Account verification failed.");
            Require(client.DefaultRequestHeaders.Authorization is null, "Authentication persisted a default Authorization header.");
        }
        foreach (var status in new[] { HttpStatusCode.Unauthorized, HttpStatusCode.Forbidden, HttpStatusCode.Redirect })
        {
            using var client = new HttpClient(new ResponseHandler(request => new HttpResponseMessage(status)
            {
                RequestMessage = request,
                Headers = { Location = new Uri("https://example.com/never-follow") },
                Content = new StringContent(first)
            }));
            await RequireSafeFailureAsync(new GitHubAccountService(client), first);
        }
        foreach (var body in new[] { "{\"login\":\"invalid/user\"}", "{\"login\":null}", "{broken" })
        {
            using var client = new HttpClient(new ResponseHandler(request => new HttpResponseMessage(HttpStatusCode.OK)
            { RequestMessage = request, Content = new StringContent(body) }));
            await RequireSafeFailureAsync(new GitHubAccountService(client), first);
        }
        Console.WriteLine("PASS: isolated Windows credential persistence/replacement/removal and bounded GitHub account authentication.");
    }

    private static async Task RequireSafeFailureAsync(GitHubAccountService service, string token)
    {
        Exception? failure = null;
        try { await service.VerifyTokenAsync(token); }
        catch (Exception ex) { failure = ex; }
        Require(failure is not null && !failure.ToString().Contains(token, StringComparison.Ordinal),
            "Invalid authentication succeeded or exposed a secret in its error.");
    }
    private static void Require(bool value, string message) { if (!value) throw new InvalidOperationException(message); }
    private sealed class ResponseHandler(Func<HttpRequestMessage, HttpResponseMessage> respond) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken) =>
            Task.FromResult(respond(request));
    }
}
