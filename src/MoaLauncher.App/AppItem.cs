using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Text.RegularExpressions;
using MoaLauncher.Core;

namespace MoaLauncher.App;

public sealed class AppItem(AppDefinition definition) : INotifyPropertyChanged
{
    public AppDefinition Definition { get; } = definition;
    private AppRelease? release;
    private string notes = "릴리스를 확인하면 새 버전의 변경 사항을 여기에 표시합니다.";
    public AppRelease? Release
    {
        get => release;
        set
        {
            release = value;
            var source = value?.Notes;
            notes = string.IsNullOrWhiteSpace(source) ? "등록된 릴리스 노트가 없습니다." :
                Regex.Replace(Regex.Replace(Regex.Replace(source, @"(?m)^#{1,6}\s+", ""), @"\[([^\]]+)\]\([^)]+\)", "$1").Replace("`", ""), @"\n{3,}", "\n\n").Trim();
        }
    }
    public InstalledApp? Installed { get; set; }
    public string Name => Definition.Name;
    public string Description => Definition.Description;
    public string Monogram => Definition.Monogram;
    public string AccentColor => Definition.AccentColor;
    public string Category => Definition.Category;
    public string Repository => Definition.Repository;
    public bool NeedsConnection => string.IsNullOrWhiteSpace(Repository);
    public bool HasRepository => !NeedsConnection;
    public bool HasUpdate => Release is not null && Installed is not null && ReleaseRules.IsNewer(Release.Tag, Installed.Version);
    public bool IsInstalled => Installed is not null;
    public string VersionText => Installed is null ? "아직 설치되지 않음" : $"설치됨 · {Installed.Version}";
    public string InstalledVersion => Installed?.Version ?? "미설치";
    public string LatestVersion => Release?.Tag ?? (HasError ? "확인 필요" : "확인 전");
    public string LatestText => Release is null ? "릴리스 확인 전" : $"최신 {Release.Tag}";
    public string StatusText => Busy ? ProgressText : HasUpdate ? "업데이트 가능" : IsInstalled ? "사용 준비 완료" : NeedsConnection ? "저장소 연결 필요" : Release is not null ? "설치 가능" : "릴리스 확인 필요";
    public string StatusColor => Busy || HasUpdate ? "#5369E8" : IsInstalled ? "#43866B" : NeedsConnection ? "#9B773A" : "#7D879A";
    public string ActionText => Busy ? "작업 중…" : NeedsConnection ? "저장소 연결" : HasUpdate ? "업데이트" : IsInstalled ? "실행하기  ↗" : Release is null ? "릴리스 확인" : "설치하기  ↓";
    public bool CanAct => !Busy;
    public bool CanCancel { get; set; }
    private bool isSelected;
    public bool IsSelected { get => isSelected; set { isSelected = value; Changed(); } }
    public string Notes => notes;
    private bool busy;
    public bool Busy { get => busy; set { busy = value; Refresh(); } }
    private string progressText = "";
    public string ProgressText { get => progressText; set { progressText = value; Changed(); Changed(nameof(StatusText)); } }
    private string? error;
    public string? Error { get => error; set { error = value; Changed(); Changed(nameof(HasError)); } }
    public bool HasError => !string.IsNullOrEmpty(Error);
    private double percent;
    public double Percent { get => percent; set { percent = value; Changed(); } }
    public bool IsCached { get; set; }
    public string SourceText => IsCached ? "오프라인 캐시 · 설치 전 다시 확인됩니다" : "GitHub Releases";
    public event PropertyChangedEventHandler? PropertyChanged;
    private void Changed([CallerMemberName] string? name = null) => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
    public void Refresh() => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(null));
}
