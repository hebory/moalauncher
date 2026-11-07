namespace MoaLauncher.Core;

/// <summary>Path helpers shared by downloads and package extraction.</summary>
public static class SafePaths
{
    public static string RequireSegment(string value, string parameterName = "value")
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(value, parameterName);
        if (value is "." or ".." || value.Any(c => !(char.IsAsciiLetterOrDigit(c) || c is '-' or '_' or '.')))
            throw new ArgumentException("경로 이름에 허용되지 않는 문자가 있습니다.", parameterName);
        return value;
    }

    public static string ResolveRelative(string root, string relativePath)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(root);
        ArgumentException.ThrowIfNullOrWhiteSpace(relativePath);
        if (Path.IsPathRooted(relativePath) || relativePath.Contains(':') || relativePath.Contains('\0'))
            throw new InvalidDataException("패키지에 절대 경로가 포함되어 있습니다.");

        var parts = relativePath.Replace('\\', '/').Split('/');
        if (parts.Any(p => p.Length == 0 || p is "." or ".." || p.EndsWith(' ') || p.EndsWith('.') || p.Any(char.IsControl)))
            throw new InvalidDataException("패키지에 안전하지 않은 경로가 포함되어 있습니다.");
        if (parts.Any(IsReservedWindowsName))
            throw new InvalidDataException("패키지에 Windows 예약 파일 이름이 포함되어 있습니다.");

        var fullRoot = Path.GetFullPath(root);
        var fullPath = Path.GetFullPath(Path.Combine(fullRoot, Path.Combine(parts)));
        var prefix = fullRoot.EndsWith(Path.DirectorySeparatorChar) ? fullRoot : fullRoot + Path.DirectorySeparatorChar;
        if (!fullPath.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
            throw new InvalidDataException("패키지 경로가 설치 폴더를 벗어납니다.");
        return fullPath;
    }

    public static bool IsWithin(string root, string path)
    {
        var fullRoot = Path.GetFullPath(root).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
        var fullPath = Path.GetFullPath(path);
        return fullPath.StartsWith(fullRoot + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase);
    }

    private static bool IsReservedWindowsName(string name)
    {
        var stem = name.Split('.')[0];
        return stem.Equals("CON", StringComparison.OrdinalIgnoreCase)
            || stem.Equals("PRN", StringComparison.OrdinalIgnoreCase)
            || stem.Equals("AUX", StringComparison.OrdinalIgnoreCase)
            || stem.Equals("NUL", StringComparison.OrdinalIgnoreCase)
            || (stem.Length == 4 && (stem.StartsWith("COM", StringComparison.OrdinalIgnoreCase)
                || stem.StartsWith("LPT", StringComparison.OrdinalIgnoreCase)) && stem[3] is >= '1' and <= '9');
    }
}
