using System.Drawing;
using System.Reflection;
using System.Windows.Forms;

namespace MoaLauncher.App;

/// <summary>Owns the notification-area icon and its window and exit actions.</summary>
internal sealed class TrayIconService : IDisposable
{
    private const string IconResourceName = "MoaLauncher.TrayIcon.ico";
    private readonly Action openWindow;
    private readonly Action exitApplication;
    private readonly NotifyIcon notifyIcon;
    private readonly ContextMenuStrip contextMenu;
    private readonly Icon icon;
    private bool hiddenNoticeShown;
    private bool disposed;

    public TrayIconService(Action openWindow, Action exitApplication)
    {
        this.openWindow = openWindow ?? throw new ArgumentNullException(nameof(openWindow));
        this.exitApplication = exitApplication ?? throw new ArgumentNullException(nameof(exitApplication));
        icon = LoadIcon();

        contextMenu = new ContextMenuStrip();
        var openItem = contextMenu.Items.Add("모아 런처 열기");
        openItem.Click += (_, _) => this.openWindow();
        contextMenu.Items.Add(new ToolStripSeparator());
        var exitItem = contextMenu.Items.Add("종료");
        exitItem.Click += (_, _) => this.exitApplication();

        notifyIcon = new NotifyIcon
        {
            Icon = icon,
            Text = "Moa Launcher",
            ContextMenuStrip = contextMenu,
            Visible = true
        };
        notifyIcon.MouseClick += OnMouseClick;
    }

    /// <summary>Shows a one-time hint the first time the window is hidden to the tray.</summary>
    public void NotifyHidden()
    {
        if (disposed || hiddenNoticeShown) return;
        hiddenNoticeShown = true;
        notifyIcon.BalloonTipTitle = "Moa Launcher";
        notifyIcon.BalloonTipText = "모아 런처는 트레이에서 계속 실행됩니다. 아이콘을 눌러 다시 열거나 오른쪽 버튼 메뉴에서 종료할 수 있습니다.";
        notifyIcon.BalloonTipIcon = ToolTipIcon.Info;
        notifyIcon.ShowBalloonTip(6000);
    }

    private static Icon LoadIcon()
    {
        var stream = Assembly.GetExecutingAssembly().GetManifestResourceStream(IconResourceName)
            ?? throw new InvalidOperationException($"내장 트레이 아이콘을 찾을 수 없습니다: {IconResourceName}");
        using (stream)
        using (var resourceIcon = new Icon(stream))
            return (Icon)resourceIcon.Clone();
    }

    private void OnMouseClick(object? sender, MouseEventArgs e)
    {
        if (e.Button == MouseButtons.Left && !disposed)
            openWindow();
    }

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        notifyIcon.Visible = false;
        notifyIcon.MouseClick -= OnMouseClick;
        notifyIcon.Dispose();
        contextMenu.Dispose();
        icon.Dispose();
    }
}
