namespace MoaLauncher.Core;

/// <summary>Provides the user's GitHub read token without persisting it with app or release data.</summary>
public interface IGitHubTokenProvider
{
    string? GetToken();
}
