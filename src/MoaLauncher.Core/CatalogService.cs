using System.Text.Json;
using System.Text.RegularExpressions;

namespace MoaLauncher.Core;

public static class CatalogService
{
    public static JsonSerializerOptions JsonOptions { get; } = new()
    {
        PropertyNameCaseInsensitive = true,
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        WriteIndented = true
    };

    public static async Task<AppCatalog> LoadAsync(string path, CancellationToken cancellationToken = default)
    {
        await using var stream = File.OpenRead(path);
        var catalog = await JsonSerializer.DeserializeAsync<AppCatalog>(stream, JsonOptions, cancellationToken)
                      ?? throw new InvalidDataException("카탈로그 파일이 비어 있거나 형식이 올바르지 않습니다.");
        Validate(catalog);
        return catalog;
    }

    public static async Task SaveAsync(string path, AppCatalog catalog, CancellationToken cancellationToken = default)
    {
        Validate(catalog);
        var fullPath = Path.GetFullPath(path);
        Directory.CreateDirectory(Path.GetDirectoryName(fullPath)!);
        var tempPath = fullPath + ".tmp-" + Guid.NewGuid().ToString("N");
        try
        {
            await using (var stream = new FileStream(tempPath, FileMode.CreateNew, FileAccess.Write, FileShare.None,
                             16 * 1024, FileOptions.Asynchronous | FileOptions.WriteThrough))
            {
                await JsonSerializer.SerializeAsync(stream, catalog, JsonOptions, cancellationToken);
                await stream.FlushAsync(cancellationToken);
            }
            File.Move(tempPath, fullPath, overwrite: true);
        }
        finally
        {
            if (File.Exists(tempPath)) File.Delete(tempPath);
        }
    }

    public static void Validate(AppCatalog catalog)
    {
        ArgumentNullException.ThrowIfNull(catalog);
        if (catalog.SchemaVersion != 1) throw new InvalidDataException("지원하지 않는 카탈로그 버전입니다.");
        if (catalog.Apps is null || catalog.Apps.Count > 100) throw new InvalidDataException("앱 목록은 100개 이하여야 합니다.");
        var ids = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var app in catalog.Apps)
        {
            if (app is null) throw new InvalidDataException("앱 항목이 비어 있습니다.");
            if (string.IsNullOrEmpty(app.Id) || !Regex.IsMatch(app.Id, "^[a-z0-9][a-z0-9-]{0,63}$")) throw new InvalidDataException("앱 ID 형식이 올바르지 않습니다.");
            if (!ids.Add(app.Id)) throw new InvalidDataException($"중복된 앱 ID입니다: {app.Id}");
            CheckText(app.Name, "앱 이름", 80, required: true);
            CheckText(app.Description, "앱 설명", 500, required: false);
            CheckText(app.Category, "앱 분류", 40, required: true);
            CheckText(app.Monogram, "앱 표시 문자", 4, required: true);
            if (app.AccentColor is null || !Regex.IsMatch(app.AccentColor, "^#[0-9a-fA-F]{6}$")) throw new InvalidDataException($"앱 색상 형식이 올바르지 않습니다: {app.Id}");
            if (app.Repository is null || (app.Repository.Length != 0 && !IsRepository(app.Repository))) throw new InvalidDataException($"GitHub 저장소 형식이 올바르지 않습니다: {app.Id}");
            if (!IsOptionalImageUrl(app.IconUrl, app.Repository)) throw new InvalidDataException($"앱 아이콘 URL 형식이 올바르지 않습니다: {app.Id}");
            if (!IsOptionalImageUrl(app.CoverUrl, app.Repository)) throw new InvalidDataException($"앱 커버 URL 형식이 올바르지 않습니다: {app.Id}");
            if (app.Packages is null || app.Packages.Count is 0 or > 12) throw new InvalidDataException($"앱 패키지 목록이 올바르지 않습니다: {app.Id}");
            var architectures = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (var package in app.Packages)
            {
                if (package is null) throw new InvalidDataException($"앱 패키지 항목이 비어 있습니다: {app.Id}");
                if (package.Architecture is not ("x64" or "x86" or "arm64")) throw new InvalidDataException($"지원하지 않는 프로세서 형식입니다: {app.Id}");
                if (!architectures.Add(package.Architecture)) throw new InvalidDataException($"중복된 프로세서 패키지입니다: {app.Id}");
                if (package.Kind is not ("zip" or "inno")) throw new InvalidDataException($"지원하지 않는 설치 방식입니다: {app.Id}");
                if (string.IsNullOrEmpty(package.AssetPattern) || package.AssetPattern.Length > 160 || package.AssetPattern.IndexOfAny(['/', '\\', ':']) >= 0 || package.AssetPattern.Contains("..", StringComparison.Ordinal))
                    throw new InvalidDataException($"릴리스 파일 패턴이 올바르지 않습니다: {app.Id}");
                if (!IsRelativePath(package.ExecutablePath) || !string.Equals(Path.GetExtension(package.ExecutablePath), ".exe", StringComparison.OrdinalIgnoreCase))
                    throw new InvalidDataException($"실행 파일 경로가 올바르지 않습니다: {app.Id}");
                if (package.Kind == "inno" && !IsRelativePath(package.InstallDirectory)) throw new InvalidDataException($"설치 폴더 경로가 올바르지 않습니다: {app.Id}");
                if (package.Kind == "zip" && package.InstallDirectory is not null) throw new InvalidDataException($"압축 패키지에는 설치 폴더를 지정할 수 없습니다: {app.Id}");
                var expectedExtension = package.Kind == "zip" ? ".zip" : ".exe";
                if (!string.Equals(Path.GetExtension(package.AssetPattern), expectedExtension, StringComparison.OrdinalIgnoreCase))
                    throw new InvalidDataException($"릴리스 파일 확장자가 설치 방식과 일치하지 않습니다: {app.Id}");
            }
        }
    }

    internal static bool IsRepository(string? repository) =>
        repository is not null && Regex.IsMatch(repository, "^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})/[A-Za-z0-9_.-]{1,100}$");

    private static bool IsOptionalImageUrl(string? value, string? repository)
    {
        if (string.IsNullOrEmpty(value)) return true;
        if (value.Length > 2048 || value.Any(char.IsControl) || !Uri.TryCreate(value, UriKind.Absolute, out var uri)) return false;
        if (!string.Equals(uri.Scheme, Uri.UriSchemeHttps, StringComparison.OrdinalIgnoreCase) ||
            !string.Equals(uri.IdnHost, "raw.githubusercontent.com", StringComparison.OrdinalIgnoreCase) ||
            !uri.IsDefaultPort || uri.UserInfo.Length != 0 || uri.Query.Length != 0 || uri.Fragment.Length != 0)
            return false;

        var repositoryParts = repository?.Split('/');
        if (repositoryParts is not { Length: 2 }) return false;
        try
        {
            var pathParts = uri.AbsolutePath.Split('/', StringSplitOptions.RemoveEmptyEntries)
                .Select(Uri.UnescapeDataString).ToArray();
            if (pathParts.Length < 4 ||
                !string.Equals(pathParts[0], repositoryParts[0], StringComparison.OrdinalIgnoreCase) ||
                !string.Equals(pathParts[1], repositoryParts[1], StringComparison.OrdinalIgnoreCase))
                return false;
            if (pathParts.Any(part => string.IsNullOrWhiteSpace(part) || part is "." or ".." ||
                                      part.Contains('/') || part.Contains('\\') || part.Any(char.IsControl)))
                return false;

            var extension = Path.GetExtension(pathParts[^1]);
            return extension.Equals(".png", StringComparison.OrdinalIgnoreCase) ||
                   extension.Equals(".jpg", StringComparison.OrdinalIgnoreCase) ||
                   extension.Equals(".jpeg", StringComparison.OrdinalIgnoreCase);
        }
        catch (UriFormatException) { return false; }
    }

    private static void CheckText(string value, string label, int maxLength, bool required)
    {
        if (value is null || value.Length > maxLength || (required && string.IsNullOrWhiteSpace(value)) || value.Any(char.IsControl))
            throw new InvalidDataException($"{label} 길이 또는 내용이 올바르지 않습니다.");
    }

    private static bool IsRelativePath(string? path)
    {
        if (string.IsNullOrWhiteSpace(path) || path.Length > 240 || Path.IsPathRooted(path) || path.Contains('\\') || path.Contains(':')) return false;
        return path.Split('/').All(part => part.Length > 0 && part is not ("." or "..") &&
            !part.EndsWith(' ') && !part.EndsWith('.') && !part.Any(char.IsControl) && !IsReservedWindowsName(part));
    }

    private static bool IsReservedWindowsName(string part)
    {
        var stem = part.Split('.')[0];
        return stem.Equals("CON", StringComparison.OrdinalIgnoreCase) || stem.Equals("PRN", StringComparison.OrdinalIgnoreCase) ||
               stem.Equals("AUX", StringComparison.OrdinalIgnoreCase) || stem.Equals("NUL", StringComparison.OrdinalIgnoreCase) ||
               (stem.Length == 4 && (stem.StartsWith("COM", StringComparison.OrdinalIgnoreCase) ||
                stem.StartsWith("LPT", StringComparison.OrdinalIgnoreCase)) && stem[3] is >= '1' and <= '9');
    }
}
