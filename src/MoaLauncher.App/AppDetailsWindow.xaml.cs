using System.Windows;

namespace MoaLauncher.App;

public partial class AppDetailsWindow : Window
{
    private readonly AppItem item;
    private readonly Action<AppItem> launch;
    private readonly Action<AppItem> openRepository;
    private readonly Action<AppItem> openFolder;
    private readonly Action<AppItem> edit;

    public AppDetailsWindow(AppItem item, Action<AppItem> launch,
        Action<AppItem> openRepository, Action<AppItem> openFolder, Action<AppItem> edit)
    {
        ArgumentNullException.ThrowIfNull(item);
        this.item = item;
        this.launch = launch ?? throw new ArgumentNullException(nameof(launch));
        this.openRepository = openRepository ?? throw new ArgumentNullException(nameof(openRepository));
        this.openFolder = openFolder ?? throw new ArgumentNullException(nameof(openFolder));
        this.edit = edit ?? throw new ArgumentNullException(nameof(edit));

        InitializeComponent();
        DataContext = item;
        Title = $"{item.Name} · Moa Launcher";
    }

    private void Repository_Click(object sender, RoutedEventArgs e)
    {
        if (item.HasRepository) openRepository(item);
    }

    private void Folder_Click(object sender, RoutedEventArgs e)
    {
        if (item.IsInstalled) openFolder(item);
    }

    private void Launch_Click(object sender, RoutedEventArgs e)
    {
        if (item.IsInstalled && item.CanAct) launch(item);
    }

    private void Edit_Click(object sender, RoutedEventArgs e)
    {
        Close();
        edit(item);
    }

    private void Close_Click(object sender, RoutedEventArgs e) => Close();
}
