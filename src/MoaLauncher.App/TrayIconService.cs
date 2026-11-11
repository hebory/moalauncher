using System.ComponentModel;
using System.Drawing;
using System.Reflection;
using System.Windows.Forms;

namespace MoaLauncher.App;

/// <summary>Owns the notification-area icon and its window, update, and exit actions.</summary>
internal sealed class TrayIconService : IDisposable
{
    private const string IconResourceName = "MoaLauncher.TrayIcon.ico";
    private readonly Action openWindow;
    private readonly Action exitApplication;
    private readonly NotifyIcon notifyIcon;
    private readonly ContextMenuStrip contextMenu;
    private readonly ToolStripItem updateAllItem;
    private readonly Func<bool> canUpdateAll;
    private readonly Icon icon;
    private readonly Action<IReadOnlyCollection<string>> openUpdates;
    private readonly HashSet<UpdateKey> announcedUpdates = new(UpdateKeyComparer.Instance);
    // NotifyIcon has no per-tip activation payload. Keep earlier apps available for late clicks too.
    private readonly HashSet<string> announcedAppIds = new(StringComparer.OrdinalIgnoreCase);
    private bool hiddenNoticeShown;
    private bool disposed;
    private bool hasAnnouncedUpdates;

    public TrayIconService(Action openWindow, Action exitApplication, Action<IReadOnlyCollection<string>> openUpdates,
        Action updateAll, Func<bool> canUpdateAll)
    {
        this.openWindow = openWindow ?? throw new ArgumentNullException(nameof(openWindow));
        this.exitApplication = exitApplication ?? throw new ArgumentNullException(nameof(exitApplication));
        this.openUpdates = openUpdates ?? throw new ArgumentNullException(nameof(openUpdates));
        ArgumentNullException.ThrowIfNull(updateAll);
        this.canUpdateAll = canUpdateAll ?? throw new ArgumentNullException(nameof(canUpdateAll));
        icon = LoadIcon();

        contextMenu = new ContextMenuStrip();
        var openItem = contextMenu.Items.Add("모아 런처 열기");
        openItem.Click += (_, _) => this.openWindow();
        updateAllItem = contextMenu.Items.Add("모두 업데이트");
        updateAllItem.Enabled = false;
        updateAllItem.Click += (_, _) =>
        {
            // State may change while the menu is open (for example, a periodic check can start).
            if (!disposed && this.canUpdateAll()) updateAll();
        };
        contextMenu.Opening += OnMenuOpening;
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
        notifyIcon.BalloonTipClicked += OnBalloonTipClicked;
    }

    /// <summary>Shows a one-time hint the first time the window is hidden to the tray.</summary>
    public void NotifyHidden()
    {
        if (disposed || hiddenNoticeShown || hasAnnouncedUpdates) return;
        if (TryShowBalloonTip("Moa Launcher", "모아 런처는 트레이에서 계속 실행됩니다. 아이콘을 눌러 다시 열거나 오른쪽 버튼 메뉴에서 종료할 수 있습니다.", ToolTipIcon.Info))
            hiddenNoticeShown = true;
    }

    public void NotifyUpdates(IEnumerable<AppItem> items)
    {
        ArgumentNullException.ThrowIfNull(items);
        if (disposed) return;

        var candidates = items
            .Where(item => item.HasUpdate && !item.IsCached && !item.HasError && !item.Busy && !item.NeedsConnection && item.Release is not null)
            .Select(item => (Item: item, Release: item.Release!))
            .OrderBy(pair => pair.Item.Name, StringComparer.OrdinalIgnoreCase)
            .ThenBy(pair => pair.Item.Definition.Id, StringComparer.OrdinalIgnoreCase)
            .Where(pair => !announcedUpdates.Contains(new UpdateKey(pair.Item.Definition.Id, pair.Item.Repository, pair.Release.Tag)))
            .ToArray();
        if (candidates.Length == 0) return;

        var title = "프로그램 업데이트 · Moa Launcher";
        var message = candidates.Length == 1
            ? $"{candidates[0].Item.Name} {candidates[0].Release.Tag} 업데이트를 사용할 수 있습니다."
            : $"{candidates.Length}개 프로그램 업데이트: {string.Join(", ", candidates.Select(candidate => candidate.Item.Name))}";
        const string clickHint = "\n클릭하여 런처에서 업데이트하세요.";
        if (!TryShowBalloonTip(title, Truncate(message, 255 - clickHint.Length) + clickHint, ToolTipIcon.Info)) return;

        foreach (var candidate in candidates)
        {
            announcedUpdates.Add(new UpdateKey(candidate.Item.Definition.Id, candidate.Item.Repository, candidate.Release.Tag));
            announcedAppIds.Add(candidate.Item.Definition.Id);
        }
        hasAnnouncedUpdates = true;
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

    private void OnMenuOpening(object? sender, CancelEventArgs e) =>
        updateAllItem.Enabled = !disposed && canUpdateAll();

    private void OnBalloonTipClicked(object? sender, EventArgs e)
    {
        if (disposed) return;
        if (hasAnnouncedUpdates && announcedAppIds.Count > 0)
            openUpdates(announcedAppIds.Order(StringComparer.OrdinalIgnoreCase).ToArray());
        else
            openWindow();
    }

    private bool TryShowBalloonTip(string title, string text, ToolTipIcon tipIcon)
    {
        if (disposed) return false;
        try
        {
            notifyIcon.BalloonTipTitle = Truncate(title, 63);
            notifyIcon.BalloonTipText = Truncate(text, 255);
            notifyIcon.BalloonTipIcon = tipIcon;
            notifyIcon.ShowBalloonTip(6000);
            return true;
        }
        catch (Exception ex)
        {
            try { App.Log(ex); }
            catch { }
            return false;
        }
    }

    private static string Truncate(string value, int maximumLength)
    {
        if (value.Length <= maximumLength) return value;
        var length = maximumLength - 1;
        if (length > 0 && char.IsHighSurrogate(value[length - 1])) length--;
        return value[..length] + "…";
    }

    private readonly record struct UpdateKey(string AppId, string Repository, string Version);

    private sealed class UpdateKeyComparer : IEqualityComparer<UpdateKey>
    {
        public static UpdateKeyComparer Instance { get; } = new();

        public bool Equals(UpdateKey left, UpdateKey right) =>
            string.Equals(left.AppId, right.AppId, StringComparison.OrdinalIgnoreCase) &&
            string.Equals(left.Repository, right.Repository, StringComparison.OrdinalIgnoreCase) &&
            string.Equals(left.Version, right.Version, StringComparison.Ordinal);

        public int GetHashCode(UpdateKey key) => HashCode.Combine(
            StringComparer.OrdinalIgnoreCase.GetHashCode(key.AppId),
            StringComparer.OrdinalIgnoreCase.GetHashCode(key.Repository),
            StringComparer.Ordinal.GetHashCode(key.Version));
    }

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        notifyIcon.Visible = false;
        notifyIcon.MouseClick -= OnMouseClick;
        notifyIcon.BalloonTipClicked -= OnBalloonTipClicked;
        notifyIcon.Dispose();
        contextMenu.Opening -= OnMenuOpening;
        contextMenu.Dispose();
        icon.Dispose();
    }
}
