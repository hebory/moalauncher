param([string]$RepositoryRoot = (Split-Path $PSScriptRoot -Parent))

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName WindowsBase, PresentationCore, PresentationFramework
$root = [IO.Path]::GetFullPath($RepositoryRoot)
[xml]$source = Get-Content -LiteralPath (Join-Path $root 'icon.svg') -Raw
$svg = $source.DocumentElement
$rectangle = $svg.SelectSingleNode("*[local-name()='rect']")
$symbol = $svg.SelectSingleNode("*[local-name()='path']")
$stops = $svg.SelectNodes("*[local-name()='defs']/*[local-name()='linearGradient']/*[local-name()='stop']")
$geometry = [Windows.Media.Geometry]::Parse($symbol.GetAttribute('d'))
$geometry.Freeze()
$gradient = [Windows.Media.LinearGradientBrush]::new()
$gradient.StartPoint = [Windows.Point]::new(0, 0)
$gradient.EndPoint = [Windows.Point]::new(1, 1)
foreach ($stop in $stops) {
    $color = [Windows.Media.ColorConverter]::ConvertFromString($stop.GetAttribute('stop-color'))
    $offset = [double]::Parse($stop.GetAttribute('offset'), [Globalization.CultureInfo]::InvariantCulture)
    $gradient.GradientStops.Add([Windows.Media.GradientStop]::new($color, $offset))
}
$gradient.Freeze()
$pen = [Windows.Media.Pen]::new([Windows.Media.Brushes]::White, [double]$symbol.GetAttribute('stroke-width'))
$pen.StartLineCap = [Windows.Media.PenLineCap]::Round
$pen.EndLineCap = [Windows.Media.PenLineCap]::Round
$pen.LineJoin = [Windows.Media.PenLineJoin]::Round
$pen.Freeze()

function Render-IconPng([int]$Size) {
    $visual = [Windows.Media.DrawingVisual]::new()
    $drawing = $visual.RenderOpen()
    $drawing.PushTransform([Windows.Media.ScaleTransform]::new($Size / 512.0, $Size / 512.0))
    $rect = [Windows.Rect]::new([double]$rectangle.GetAttribute('x'), [double]$rectangle.GetAttribute('y'),
        [double]$rectangle.GetAttribute('width'), [double]$rectangle.GetAttribute('height'))
    $radius = [double]$rectangle.GetAttribute('rx')
    $drawing.DrawRoundedRectangle($gradient, $null, $rect, $radius, $radius)
    $drawing.DrawGeometry($null, $pen, $geometry)
    $drawing.Pop()
    $drawing.Close()
    $bitmap = [Windows.Media.Imaging.RenderTargetBitmap]::new($Size, $Size, 96, 96, [Windows.Media.PixelFormats]::Pbgra32)
    $bitmap.Render($visual)
    $bitmap.Freeze()
    $encoder = [Windows.Media.Imaging.PngBitmapEncoder]::new()
    $encoder.Frames.Add([Windows.Media.Imaging.BitmapFrame]::Create($bitmap))
    $stream = [IO.MemoryStream]::new()
    try { $encoder.Save($stream); return ,$stream.ToArray() } finally { $stream.Dispose() }
}

[IO.File]::WriteAllBytes((Join-Path $root 'icon.png'), (Render-IconPng 512))
$sizes = @(16, 24, 32, 48, 64, 128, 256)
$frames = @($sizes | ForEach-Object { ,(Render-IconPng $_) })
$iconPath = Join-Path $root 'src\MoaLauncher.App\Assets\launcher.ico'
$stream = [IO.File]::Create($iconPath)
$writer = [IO.BinaryWriter]::new($stream)
try {
    $writer.Write([uint16]0)
    $writer.Write([uint16]1)
    $writer.Write([uint16]$sizes.Count)
    $offset = 6 + 16 * $sizes.Count
    for ($index = 0; $index -lt $sizes.Count; $index++) {
        $dimension = if ($sizes[$index] -eq 256) { 0 } else { $sizes[$index] }
        $writer.Write([byte]$dimension)
        $writer.Write([byte]$dimension)
        $writer.Write([byte]0)
        $writer.Write([byte]0)
        $writer.Write([uint16]1)
        $writer.Write([uint16]32)
        $writer.Write([uint32]$frames[$index].Length)
        $writer.Write([uint32]$offset)
        $offset += $frames[$index].Length
    }
    foreach ($frame in $frames) { $writer.Write([byte[]]$frame) }
} finally { $writer.Dispose(); $stream.Dispose() }
Write-Output 'Created icon.png and Assets/launcher.ico (16, 24, 32, 48, 64, 128, 256 px).'
