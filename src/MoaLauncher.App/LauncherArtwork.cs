using System.Windows.Media.Imaging;

namespace MoaLauncher.App;

internal static class LauncherArtwork
{
    internal static BitmapSource LoadIcon() => Load("launcher-icon.png");

    internal static BitmapSource LoadCover() => Load("launcher-cover.png");

    private static BitmapSource Load(string resourceName)
    {
        var uri = new Uri($"pack://application:,,,/MoaLauncher;component/Assets/{resourceName}", UriKind.Absolute);
        var bitmap = new BitmapImage();
        bitmap.BeginInit();
        bitmap.CacheOption = BitmapCacheOption.OnLoad;
        bitmap.UriSource = uri;
        bitmap.EndInit();
        bitmap.Freeze();
        return bitmap;
    }
}
