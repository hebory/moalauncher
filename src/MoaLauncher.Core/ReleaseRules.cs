using System.Globalization;
using System.Runtime.InteropServices;
using System.Text.RegularExpressions;

namespace MoaLauncher.Core;

public static class ReleaseRules
{
    private static readonly Regex VersionPattern = new(
        "^[vV]?(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z.-]+))?(?:\\+[0-9A-Za-z.-]+)?$",
        RegexOptions.CultureInvariant | RegexOptions.Compiled);

    public static PackageDefinition SelectPackage(AppDefinition app, Architecture? architecture = null)
    {
        ArgumentNullException.ThrowIfNull(app);
        var target = architecture ?? RuntimeInformation.OSArchitecture;
        var archName = target switch
        {
            Architecture.X64 => "x64",
            Architecture.X86 => "x86",
            Architecture.Arm64 => "arm64",
            _ => throw new InvalidOperationException("현재 프로세서에서 지원하는 Moa 앱 패키지가 없습니다.")
        };
        return app.Packages.SingleOrDefault(p => string.Equals(p.Architecture, archName, StringComparison.OrdinalIgnoreCase))
               ?? throw new InvalidOperationException($"이 앱은 {archName} 프로세서를 지원하지 않습니다.");
    }

    public static ReleaseAsset SelectAsset(AppRelease release, PackageDefinition package)
    {
        ArgumentNullException.ThrowIfNull(release);
        ArgumentNullException.ThrowIfNull(package);
        var matcher = "^" + Regex.Escape(package.AssetPattern).Replace("\\*", ".*").Replace("\\?", ".") + "$";
        var matches = release.Assets.Where(asset => Regex.IsMatch(asset.Name, matcher, RegexOptions.IgnoreCase | RegexOptions.CultureInvariant)).ToArray();
        return matches.Length switch
        {
            1 => matches[0],
            0 => throw new InvalidDataException($"릴리스에서 '{package.AssetPattern}' 파일을 찾을 수 없습니다."),
            _ => throw new InvalidDataException($"릴리스에서 '{package.AssetPattern}'에 맞는 파일이 여러 개 발견되었습니다.")
        };
    }

    public static bool IsNewer(string latest, string installed)
    {
        if (!TryParseVersion(latest, out var newer) || !TryParseVersion(installed, out var older)) return false;
        var comparison = newer.Major.CompareTo(older.Major);
        if (comparison == 0) comparison = newer.Minor.CompareTo(older.Minor);
        if (comparison == 0) comparison = newer.Patch.CompareTo(older.Patch);
        if (comparison != 0) return comparison > 0;
        if (newer.Prerelease is null) return older.Prerelease is not null;
        if (older.Prerelease is null) return false;
        return ComparePrerelease(newer.Prerelease, older.Prerelease) > 0;
    }

    public static bool ValidateAssetUrl(string repository, Uri url)
    {
        if (!CatalogService.IsRepository(repository) || url is null || !url.IsAbsoluteUri ||
            !string.Equals(url.Scheme, Uri.UriSchemeHttps, StringComparison.OrdinalIgnoreCase) ||
            !string.Equals(url.IdnHost, "github.com", StringComparison.OrdinalIgnoreCase) || !url.IsDefaultPort ||
            !string.IsNullOrEmpty(url.UserInfo) || !string.IsNullOrEmpty(url.Query) || !string.IsNullOrEmpty(url.Fragment)) return false;
        var repoParts = repository.Split('/');
        var rawParts = url.AbsolutePath.Split('/', StringSplitOptions.RemoveEmptyEntries);
        if (rawParts.Length != 6) return false;
        var pathParts = rawParts.Select(Uri.UnescapeDataString).ToArray();
        return string.Equals(pathParts[0], repoParts[0], StringComparison.OrdinalIgnoreCase) &&
               string.Equals(pathParts[1], repoParts[1], StringComparison.OrdinalIgnoreCase) &&
               pathParts[2] == "releases" && pathParts[3] == "download" &&
               pathParts.Skip(4).All(part => part.Length > 0 && part is not ("." or "..") && !part.Contains('/') && !part.Contains('\\'));
    }

    private static bool TryParseVersion(string value, out ParsedVersion version)
    {
        version = default;
        if (string.IsNullOrWhiteSpace(value)) return false;
        var match = VersionPattern.Match(value.Trim());
        if (!match.Success) return false;
        if (match.Groups[4].Success && match.Groups[4].Value.Split('.').Any(part => part.Length == 0 ||
                (part.Length > 1 && part.All(char.IsDigit) && part[0] == '0'))) return false;
        if (!long.TryParse(match.Groups[1].Value, NumberStyles.None, CultureInfo.InvariantCulture, out var major) ||
            !long.TryParse(match.Groups[2].Value, NumberStyles.None, CultureInfo.InvariantCulture, out var minor) ||
            !long.TryParse(match.Groups[3].Value, NumberStyles.None, CultureInfo.InvariantCulture, out var patch)) return false;
        version = new ParsedVersion(major, minor, patch, match.Groups[4].Success ? match.Groups[4].Value : null);
        return true;
    }

    private static int ComparePrerelease(string left, string right)
    {
        var l = left.Split('.');
        var r = right.Split('.');
        for (var i = 0; i < Math.Min(l.Length, r.Length); i++)
        {
            var ln = long.TryParse(l[i], NumberStyles.None, CultureInfo.InvariantCulture, out var li);
            var rn = long.TryParse(r[i], NumberStyles.None, CultureInfo.InvariantCulture, out var ri);
            var comparison = ln && rn ? li.CompareTo(ri) : ln ? -1 : rn ? 1 : string.Compare(l[i], r[i], StringComparison.Ordinal);
            if (comparison != 0) return comparison;
        }
        return l.Length.CompareTo(r.Length);
    }

    private readonly record struct ParsedVersion(long Major, long Minor, long Patch, string? Prerelease);
}
