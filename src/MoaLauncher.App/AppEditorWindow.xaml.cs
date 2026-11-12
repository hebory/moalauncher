using System.Windows;
using System.Windows.Controls;
using MoaLauncher.Core;

namespace MoaLauncher.App;

public partial class AppEditorWindow : Window
{
    private readonly AppDefinition? original;
    public AppDefinition? Result { get; private set; }
    public AppEditorWindow(AppDefinition? app)
    {
        InitializeComponent();
        original = app;
        if (app is null) return;
        Heading.Text = app.Name + " 설정";
        NameInput.Text = app.Name;
        IdInput.Text = app.Id;
        IdInput.IsReadOnly = true;
        RepositoryInput.Text = app.Repository;
        PrivateRepositoryCheck.IsChecked = app.IsPrivate;
        DescriptionInput.Text = app.Description;
        var package = app.Packages.FirstOrDefault();
        if (package is null) return;
        PatternInput.Text = package.AssetPattern;
        ExecutableInput.Text = package.ExecutablePath;
        DirectoryInput.Text = package.InstallDirectory ?? "";
        KindInput.SelectedIndex = package.Kind == "inno" ? 1 : 0;
        ArchitectureInput.SelectedIndex = package.Architecture switch { "arm64" => 1, "x86" => 2, _ => 0 };
    }
    private void Kind_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (InstallDirectoryPanel is not null) InstallDirectoryPanel.Visibility = KindInput.SelectedIndex == 1 ? Visibility.Visible : Visibility.Collapsed;
    }
    private void Save_Click(object sender, RoutedEventArgs e)
    {
        try
        {
            var repository = RepositoryInput.Text.Trim().TrimEnd('/');
            if (repository.StartsWith("https://github.com/", StringComparison.OrdinalIgnoreCase)) repository = repository[19..];
            if (repository.EndsWith(".git", StringComparison.OrdinalIgnoreCase)) repository = repository[..^4];
            if (repository.Length == 0) throw new ArgumentException("GitHub 저장소를 입력하세요.");
            var name = NameInput.Text.Trim();
            var package = new PackageDefinition
            {
                Architecture = ((ComboBoxItem)ArchitectureInput.SelectedItem).Content.ToString()!,
                Kind = KindInput.SelectedIndex == 1 ? "inno" : "zip",
                AssetPattern = PatternInput.Text.Trim(),
                ExecutablePath = ExecutableInput.Text.Trim(),
                InstallDirectory = KindInput.SelectedIndex == 1 ? DirectoryInput.Text.Trim() : null
            };
            // Preserve the other architectures of imported multi-platform entries.
            var packages = original?.Packages.Where(p => p.Architecture != package.Architecture).ToList() ?? [];
            packages.Add(package);
            var sameRepository = original is not null && string.Equals(original.Repository, repository, StringComparison.OrdinalIgnoreCase);
            Result = (original ?? new AppDefinition()) with
            {
                Id = IdInput.Text.Trim(), Name = name, Description = DescriptionInput.Text.Trim(), Repository = repository,
                IsPrivate = PrivateRepositoryCheck.IsChecked == true,
                Monogram = original?.Monogram ?? (name.Length > 0 ? name[..1] : "M"), Packages = packages,
                IconUrl = sameRepository ? original!.IconUrl : null,
                CoverUrl = sameRepository ? original!.CoverUrl : null
            };
            CatalogService.Validate(new AppCatalog { Apps = [Result] });
            DialogResult = true;
        }
        catch (Exception ex) { Result = null; ErrorText.Text = ex.Message; }
    }
}
