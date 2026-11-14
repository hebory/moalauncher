using System.Globalization;
using System.Windows.Data;

namespace MoaLauncher.App;

/// <summary>Keeps tiles wide enough to read while filling the available row.</summary>
public sealed class GridCardWidthConverter : IValueConverter
{
    public object Convert(object value, Type targetType, object parameter, CultureInfo culture)
    {
        if (value is not double width || !double.IsFinite(width) || width <= 0) return 280d;
        var columns = Math.Max(1, Math.Floor(width / 280));
        // Round down so layout rounding cannot push the last tile onto the next row.
        return Math.Floor(width / columns);
    }

    public object ConvertBack(object value, Type targetType, object parameter, CultureInfo culture) =>
        throw new NotSupportedException();
}
