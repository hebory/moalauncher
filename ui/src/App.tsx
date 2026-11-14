import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Search,
  Star,
  Grid2X2,
  List,
  RefreshCw,
  Download,
  ArrowUpRight,
  Settings as SettingsIcon,
  Plus,
  X,
  FolderOpen,
  ExternalLink,
  ShieldCheck,
  ChevronRight,
  Check,
  LoaderCircle,
} from "lucide-react";
import launcherIcon from "../../icon.png";
import launcherCover from "../../cover.png";
import type {
  AppView,
  AppDefinition,
  Snapshot,
  Settings,
  Artwork,
} from "./types";
import { filterApps, hasUpdate, updateOrder, type Filter } from "./library";

const emptyDefinition = (): AppDefinition => ({
  id: "",
  name: "",
  description: "",
  category: "유틸리티",
  monogram: "M",
  accentColor: "#6964e8",
  repository: "",
  isPrivate: false,
  packages: [
    { architecture: "x64", kind: "zip", assetPattern: "", executablePath: "" },
  ],
});
function errorText(error: unknown) {
  return typeof error === "string"
    ? error
    : "작업을 완료하지 못했습니다. 다시 시도해 주세요.";
}
function App() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null),
    [error, setError] = useState(""),
    [page, setPage] = useState<"library" | "settings">("library"),
    [filter, setFilter] = useState<Filter>("all"),
    [search, setSearch] = useState(""),
    [selected, setSelected] = useState<string | null>(null),
    [editor, setEditor] = useState<AppDefinition | null>(null),
    [editingExisting, setEditingExisting] = useState(false),
    [imported, setImported] = useState<{
      schemaVersion: number;
      apps: AppDefinition[];
    } | null>(null),
    [art, setArt] = useState<Record<string, Artwork>>({}),
    [pending, setPending] = useState(false),
    [favoritePending, setFavoritePending] = useState(false),
    [password, setPassword] = useState("");
  const favoritesRef = useRef<string[]>([]);
  const favoriteLock = useRef(false);
  useEffect(() => {
    let active = true;
    const unlisteners: (() => void)[] = [];
    const accept = (value: Snapshot) => {
      if (active) {
        favoritesRef.current = value.settings.favoriteAppIds;
        setSnapshot(value);
      }
    };
    const showUpdates = (ids: string[]) => {
      if (active) {
        setPage("library");
        setFilter("updates");
        setSearch("");
        setSelected(null);
        setSnapshot((value) =>
          value ? { ...value, notificationIds: ids } : value,
        );
      }
    };
    // Register first, then request a snapshot so startup checks cannot be missed.
    void Promise.all([
      listen<Snapshot>("launcher-state", (e) => accept(e.payload)),
      listen<string[]>("launcher-show-updates", (e) => showUpdates(e.payload)),
      listen<AppView["progress"]>("install-progress", (e) => {
        if (active && e.payload) {
          const progress = e.payload;
          setSnapshot((value) =>
            value
              ? {
                  ...value,
                  apps: value.apps.map((app) =>
                    app.definition.id === progress.appId
                      ? { ...app, progress }
                      : app,
                  ),
                }
              : value,
          );
        }
      }),
    ])
      .then(async (listeners) => {
        if (!active) {
          listeners.forEach((stop) => stop());
          return;
        }
        unlisteners.push(...listeners);
        const value = await invoke<Snapshot>("snapshot");
        accept(value);
        if (value.notificationIds.length) showUpdates(value.notificationIds);
      })
      .catch((e) => active && setError(errorText(e)));
    return () => {
      active = false;
      unlisteners.forEach((stop) => stop());
    };
  }, []);
  const artKey = snapshot
    ? JSON.stringify([
        snapshot.authGeneration,
        snapshot.apps.map((app) => [
          app.definition.id,
          app.definition.repository,
          app.definition.iconUrl,
          app.definition.coverUrl,
        ]),
      ])
    : "";
  useEffect(() => {
    if (!snapshot) return;
    let active = true;
    setArt({});
    for (const app of snapshot.apps) {
      if (app.isLauncher) continue;
      void invoke<Artwork>("load_artwork", { id: app.definition.id })
        .then((images) => {
          if (active)
            setArt((current) => ({ ...current, [app.definition.id]: images }));
        })
        .catch(() => {});
    }
    return () => {
      active = false;
    };
  }, [artKey]);
  useEffect(() => {
    const escape = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (editor) setEditor(null);
        else if (imported) setImported(null);
        else setSelected(null);
      }
    };
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  }, [editor, imported]);
  async function action(command: string, args?: Record<string, unknown>) {
    setError("");
    try {
      return await invoke(command, args);
    } catch (e) {
      setError(errorText(e));
      return undefined;
    }
  }
  async function saveSettings(patch: Partial<Settings>) {
    await action("update_settings", { patch });
  }
  async function toggleFavorite(id: string) {
    if (favoriteLock.current) return;
    favoriteLock.current = true;
    setFavoritePending(true);
    const current = favoritesRef.current;
    const next = current.includes(id)
      ? current.filter((value) => value !== id)
      : [...current, id];
    try { await saveSettings({ favoriteAppIds: next }); }
    finally { favoriteLock.current = false; setFavoritePending(false); }
  }
  function navigate(next: Filter) {
    setFilter(next);
    setPage("library");
  }
  function edit(app?: AppDefinition) {
    setSelected(null);
    setEditor(app ? structuredClone(app) : emptyDefinition());
    setEditingExisting(!!app);
  }
  async function primary(app: AppView) {
    if (!app.definition.repository) {
      edit(app.definition);
      return;
    }
    if (app.installed && !hasUpdate(app) && !app.isLauncher) {
      await action("launch_app", { id: app.definition.id });
      return;
    }
    if (!app.release || (app.isLauncher && !hasUpdate(app))) {
      await action("check_updates");
      return;
    }
    await action("install_app", { id: app.definition.id });
  }
  async function connect() {
    const token = password;
    setPassword("");
    setPending(true);
    try {
      await action("connect_github", { token });
    } finally {
      setPending(false);
    }
  }
  async function importCatalog() {
    setPending(true);
    try {
      const result = await action("import_catalog");
      if (result) setImported(result as typeof imported);
    } finally {
      setPending(false);
    }
  }
  if (!snapshot)
    return (
      <main className="loading">
        <img src={launcherIcon} alt="" />
        <LoaderCircle className="spin" />
        <p>{error || "내 도구를 불러오고 있습니다…"}</p>
      </main>
    );
  const busy =
    snapshot.checking ||
    snapshot.batchUpdating ||
    snapshot.apps.some((app) => app.busy) ||
    pending;
  const updates = updateOrder(snapshot.apps).length;
  const visible = filterApps(
    snapshot.apps,
    filter,
    search,
    snapshot.settings.favoriteAppIds,
    snapshot.notificationIds,
  );
  const detail = snapshot.apps.find((app) => app.definition.id === selected);
  function images(app: AppView) {
    return app.isLauncher
      ? { icon: launcherIcon, cover: launcherCover }
      : art[app.definition.id] || { icon: null, cover: null };
  }
  function status(app: AppView) {
    if (app.busy) return app.progress?.stage || "새 버전 확인 중…";
    if (hasUpdate(app)) return "업데이트 가능";
    if (app.error) return "확인 필요";
    if (app.installed) return "사용 준비 완료";
    if (!app.definition.repository) return "저장소 연결 필요";
    return app.release ? "설치 가능" : "릴리스 확인 전";
  }
  function buttonText(app: AppView) {
    if (app.busy) return "작업 중…";
    if (!app.definition.repository) return "저장소 연결";
    if (app.isLauncher)
      return hasUpdate(app)
        ? "업데이트 후 재시작"
        : app.release
          ? "최신 버전"
          : "업데이트 확인";
    if (app.installed && !hasUpdate(app)) return "실행하기";
    return hasUpdate(app)
      ? "업데이트"
      : app.release
        ? "설치하기"
        : "릴리스 확인";
  }
  function icon(app: AppView) {
    const image = images(app).icon;
    return image ? (
      <img
        className="app-icon"
        src={image}
        alt={`${app.definition.name} 아이콘`}
      />
    ) : (
      <span
        className="monogram"
        style={{ background: app.definition.accentColor }}
      >
        {app.definition.monogram}
      </span>
    );
  }
  return (
    <div className="app-shell">
      <header className="header">
        <a className="brand" onClick={() => navigate("all")}>
          <img src={launcherIcon} alt="" />
          <strong>moa</strong>
          <span>Launcher</span>
        </a>
        <nav>
          <button
            className={page === "library" ? "nav active" : "nav"}
            onClick={() => navigate("all")}
          >
            프로그램
          </button>
          <button
            className={page === "settings" ? "nav active" : "nav"}
            onClick={() => setPage("settings")}
          >
            <SettingsIcon size={15} />
            설정
          </button>
        </nav>
      </header>
      <main className="main">
        {snapshot.updateResult && <div className={snapshot.updateResult.success?"batch-notice":"global-error"} role="status"><span>{snapshot.updateResult.message}</span><button aria-label="업데이트 결과 닫기" onClick={()=>void action('dismiss_update_result')}><X size={15}/></button></div>}
        {error && (
          <div className="global-error" role="alert">
            <span>{error}</span>
            <button aria-label="오류 안내 닫기" onClick={() => setError("")}>
              <X size={16} />
            </button>
          </div>
        )}
        {page === "library" ? (
          <>
            <div className="page-heading">
              <div>
                <span className="eyebrow">MY WORKSPACE</span>
                <h1>
                  {filter === "favorites"
                    ? "즐겨찾기"
                    : filter === "updates"
                      ? "업데이트"
                      : "내 프로그램"}
                </h1>
                <p>
                  {visible.length}개 도구
                  {filter === "favorites" ? " · 자주 쓰는 도구를 모았어요" : ""}
                </p>
              </div>
              <div className="heading-actions">
                <button
                  className="primary"
                  disabled={busy || !updates}
                  onClick={() => void action("update_all_apps")}
                >
                  모두 업데이트{updates ? ` (${updates})` : ""}
                </button>
                <button
                  disabled={busy}
                  onClick={() => void action("check_updates")}
                >
                  <RefreshCw
                    size={14}
                    className={snapshot.checking ? "spin" : ""}
                  />
                  업데이트 확인
                </button>
                <button disabled={busy} onClick={() => edit()}>
                  <Plus size={15} />
                  프로그램 추가
                </button>
              </div>
            </div>
            <div className="toolbar">
              <div className="filters">
                {(
                  [
                    ["all", "전체"],
                    ["favorites", "즐겨찾기"],
                    ["installed", "설치됨"],
                    ["updates", `업데이트${updates ? ` ${updates}` : ""}`],
                  ] as [Filter, string][]
                ).map(([id, label]) => (
                  <button
                    key={id}
                    className={filter === id ? "filter selected" : "filter"}
                    onClick={() => navigate(id)}
                  >
                    {id === "favorites" && <Star size={13} />} {label}
                  </button>
                ))}
              </div>
              <div className="search-tools">
                <div className="view-switch">
                  <button
                    aria-label="리스트 보기"
                    aria-pressed={!snapshot.settings.useGridView}
                    className={!snapshot.settings.useGridView ? "selected" : ""}
                    onClick={() => void saveSettings({ useGridView: false })}
                  >
                    <List size={16} />
                  </button>
                  <button
                    aria-label="그리드 보기"
                    aria-pressed={snapshot.settings.useGridView}
                    className={snapshot.settings.useGridView ? "selected" : ""}
                    onClick={() => void saveSettings({ useGridView: true })}
                  >
                    <Grid2X2 size={16} />
                  </button>
                </div>
                <label className="search">
                  <Search size={16} />
                  <input
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    placeholder="프로그램 검색"
                    aria-label="프로그램 검색"
                  />
                </label>
              </div>
            </div>
            {snapshot.batchUpdating && (
              <div className="batch-notice">
                <LoaderCircle size={16} className="spin" />
                <span>
                  도구를 순서대로 업데이트합니다. 런처는 마지막에 진행합니다.
                </span>
                <button
                  onClick={() => void action("cancel_operation", { id: null })}
                >
                  남은 업데이트 취소
                </button>
              </div>
            )}
            {!visible.length ? (
              <div className="empty">
                <Star size={30} />
                <h2>
                  {search
                    ? "검색 결과가 없습니다"
                    : filter === "favorites"
                      ? "즐겨찾는 도구를 추가해 보세요"
                      : filter === "updates"
                        ? "확인된 업데이트가 없습니다"
                        : "표시할 도구가 없습니다"}
                </h2>
                <p>
                  {filter === "favorites" && !search
                    ? "도구의 별 버튼을 누르면 이곳에 모아 볼 수 있어요."
                    : "검색 조건을 바꾸거나 전체 목록을 확인해 보세요."}
                </p>
                <button
                  onClick={() => {
                    setSearch("");
                    navigate("all");
                  }}
                >
                  전체 프로그램 보기
                  <ChevronRight size={14} />
                </button>
              </div>
            ) : (
              <div
                className={
                  snapshot.settings.useGridView ? "cards grid" : "cards list"
                }
              >
                {visible.map((app) => {
                  const favorite = snapshot.settings.favoriteAppIds.includes(
                      app.definition.id,
                    ),
                    cover = images(app).cover,
                    percent = app.progress?.totalBytes
                      ? Math.min(
                          100,
                          (app.progress.bytesReceived /
                            app.progress.totalBytes) *
                            100,
                        )
                      : 0;
                  return (
                    <article className="card" key={app.definition.id}>
                      <div
                        className="card-cover"
                        style={
                          cover
                            ? { backgroundImage: `url("${cover}")` }
                            : { background: app.definition.accentColor }
                        }
                      >
                        {!cover && <span>{app.definition.monogram}</span>}
                      </div>
                      <button
                        className={`favorite ${favorite ? "is-favorite" : ""}`}
                        title={favorite ? "즐겨찾기 해제" : "즐겨찾기 추가"}
                        aria-label={`${app.definition.name} 즐겨찾기 ${favorite ? "해제" : "추가"}`}
                        aria-pressed={favorite}
                        disabled={favoritePending}
                        onClick={() => void toggleFavorite(app.definition.id)}
                      >
                        <Star
                          size={18}
                          fill={favorite ? "currentColor" : "none"}
                        />
                      </button>
                      <div className="card-content">
                        <div className="card-title">
                          {icon(app)}
                          <div>
                            <h2 title={app.definition.name}>
                              {app.definition.name}
                            </h2>
                            {app.installed && (
                              <span
                                className="version"
                                title={`설치 버전 ${app.installed.version}`}
                              >
                                v
                                {
                                  app.installed.version
                                    .replace(/^[vV]/, "")
                                    .split("+")[0]
                                }
                              </span>
                            )}
                          </div>
                        </div>
                        <p
                          className="description"
                          title={app.definition.description}
                        >
                          {app.definition.description}
                        </p>
                        {app.definition.isPrivate && (
                          <span className="private">
                            <ShieldCheck size={11} />
                            비공개 · 개인 카탈로그
                          </span>
                        )}
                        <div
                          className={`status ${hasUpdate(app) ? "update" : app.error ? "warning" : ""}`}
                        >
                          <span className="status-dot" />
                          {status(app)}
                        </div>
                        {app.busy && (
                          <div className="progress">
                            <i style={{ width: `${percent}%` }} />
                          </div>
                        )}
                        {app.error && (
                          <div className="card-error">{app.error}</div>
                        )}
                        {app.canCancel && (
                          <button
                            className="cancel"
                            onClick={() =>
                              void action("cancel_operation", {
                                id: app.definition.id,
                              })
                            }
                          >
                            다운로드 취소
                          </button>
                        )}
                      </div>
                      <div className="card-actions">
                        <button
                          className="primary"
                          disabled={
                            busy ||
                            (app.isLauncher && !!app.release && !hasUpdate(app))
                          }
                          onClick={() => void primary(app)}
                        >
                          {app.busy ? (
                            <LoaderCircle size={14} className="spin" />
                          ) : null}
                          {buttonText(app)}
                          {app.installed &&
                          !hasUpdate(app) &&
                          !app.isLauncher ? (
                            <ArrowUpRight size={14} />
                          ) : !app.installed ? (
                            <Download size={13} />
                          ) : null}
                        </button>
                        <button
                          className="detail-button"
                          onClick={() => setSelected(app.definition.id)}
                        >
                          상세 보기
                        </button>
                      </div>
                    </article>
                  );
                })}
              </div>
            )}
          </>
        ) : (
          <section className="settings-page">
            <div className="settings-heading">
              <span className="eyebrow">MAKE IT YOURS</span>
              <h1>설정</h1>
              <p>모아 런처를 내 작업 방식에 맞춰 보세요.</p>
            </div>
            <div className="settings-panel">
              <h2>시작 및 종료</h2>
              <Switch
                label="Windows 시작 시 자동 실행"
                note="로그인하면 창 없이 트레이에서 시작합니다."
                checked={snapshot.startupEnabled}
                onChange={(enabled) => void action("set_startup", { enabled })}
              />
              <p className="hint">
                창을 닫아도 트레이에서 실행됩니다. 완전히 끝내려면 트레이 메뉴의
                ‘종료’를 선택하세요.
              </p>
            </div>
            <div className="settings-panel">
              <h2>업데이트 확인</h2>
              <Switch
                label="런처를 시작할 때 새 버전 확인"
                checked={snapshot.settings.checkOnStartup}
                onChange={(checkOnStartup) =>
                  void saveSettings({ checkOnStartup })
                }
              />
              <Switch
                label="실행 중 주기적으로 새 버전 확인"
                checked={snapshot.settings.periodicChecks}
                onChange={(periodicChecks) =>
                  void saveSettings({ periodicChecks })
                }
              />
              <label className="setting-row">
                <span>
                  <strong>확인 간격</strong>
                  <small>기본 1시간 · 런처가 실행 중일 때 확인합니다.</small>
                </span>
                <select
                  disabled={!snapshot.settings.periodicChecks}
                  value={snapshot.settings.checkIntervalMinutes}
                  onChange={(e) =>
                    void saveSettings({
                      checkIntervalMinutes: Number(e.target.value),
                    })
                  }
                >
                  {[30, 60, 180, 360, 720, 1440].map((value) => (
                    <option value={value} key={value}>
                      {value < 60 ? `${value}분` : `${value / 60}시간`}
                    </option>
                  ))}
                </select>
              </label>
              <Switch
                label="업데이트 알림"
                note="새 버전을 찾으면 트레이 알림으로 알려줍니다."
                checked={snapshot.settings.updateNotifications}
                onChange={(updateNotifications) =>
                  void saveSettings({ updateNotifications })
                }
              />
            </div>
            <div className="settings-panel">
              <h2>
                GitHub 연결{" "}
                <span
                  className={
                    snapshot.githubConnected ? "connection yes" : "connection"
                  }
                >
                  {snapshot.githubConnected ? "연결됨" : "연결 안 됨"}
                </span>
              </h2>
              <p>
                비공개 저장소의 앱과 이미지를 가져옵니다. 인증은 이 PC의 Windows
                자격 증명 관리자에 보관합니다.
              </p>
              <label>
                개인 액세스 토큰
                <input
                  type="password"
                  autoComplete="off"
                  spellCheck={false}
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  placeholder={
                    snapshot.githubConnected
                      ? "새 토큰을 입력하면 교체합니다"
                      : "GitHub 개인 액세스 토큰"
                  }
                  aria-label="GitHub 개인 액세스 토큰"
                />
              </label>
              <p className="hint">
                Fine-grained 토큰에서 사용할 저장소를 선택하고 Contents 권한을
                Read-only로 지정하세요.
              </p>
              <div className="button-row">
                <button
                  className="primary"
                  disabled={busy || !password}
                  onClick={() => void connect()}
                >
                  연결{snapshot.githubConnected ? " 교체" : ""}
                </button>
                <button
                  disabled={busy || !snapshot.githubConnected}
                  onClick={() => void action("disconnect_github")}
                >
                  연결 해제
                </button>
                <button onClick={()=>void action('open_token_page')}>토큰 발급 페이지<ExternalLink size={12}/></button>
              </div>
            </div>
            <div className="settings-panel">
              <h2>프로그램과 데이터</h2>
              <p className="data-path">{snapshot.dataDirectory}</p>
              <div className="button-row">
                <button
                  onClick={() => void action("open_location", { id: null })}
                >
                  <FolderOpen size={14} />
                  데이터 폴더
                </button>
                <button disabled={busy} onClick={() => void importCatalog()}>
                  카탈로그 가져오기
                </button>
                <button
                  disabled={busy}
                  onClick={() => void action("export_catalog")}
                >
                  카탈로그 내보내기
                </button>
              </div>
              <p className="hint">
                비공개 앱과 즐겨찾기는 이 PC에만 저장됩니다. 내보내기에는 공개
                여부를 확인한 앱만 포함합니다.
              </p>
            </div>
            <div className="settings-bottom">
              <span>Moa Launcher {snapshot.version}</span>
              <button
                className="text-button"
                onClick={() => void action("exit_launcher")}
              >
                런처 종료
              </button>
            </div>
          </section>
        )}
      </main>
      <footer>
        <span>{snapshot.status}</span>
        <span>
          {snapshot.lastChecked
            ? `마지막 확인 ${snapshot.lastChecked}`
            : "아직 확인하지 않음"}
        </span>
      </footer>
      {detail && (
        <div className="modal-backdrop" onClick={() => setSelected(null)}>
          <section
            className="modal details"
            role="dialog"
            aria-modal="true"
            aria-label={`${detail.definition.name} 상세 정보`}
            onClick={(e) => e.stopPropagation()}
          >
            <header className="modal-header">
              {icon(detail)}
              <div>
                <h2>{detail.definition.name}</h2>
                <p>{detail.definition.description}</p>
              </div>
              <button
                aria-label="상세 정보 닫기"
                onClick={() => setSelected(null)}
              >
                <X size={19} />
              </button>
            </header>
            <div className="modal-scroll">
              {images(detail).cover && (
                <img
                  className="detail-cover"
                  src={images(detail).cover!}
                  alt={`${detail.definition.name} 커버 이미지`}
                />
              )}
              <div className="version-comparison">
                <div>
                  <small>현재 설치 버전</small>
                  <strong>{detail.installed?.version || "미설치"}</strong>
                </div>
                <div>
                  <small>최신 버전</small>
                  <strong>{detail.release?.tag || "확인 전"}</strong>
                </div>
              </div>
              <div className="release-heading">
                <h3>릴리스 노트</h3>
                <span>
                  {detail.definition.isPrivate
                    ? "비공개 · 개인 카탈로그"
                    : detail.isCached
                      ? "오프라인 캐시"
                      : "GitHub Releases"}
                </span>
              </div>
              <pre className="release-notes">
                {detail.release?.notes ||
                  "업데이트 확인을 누르면 배포 정보를 가져옵니다."}
              </pre>
              <div className="button-row">
                <button
                  onClick={() =>
                    void action("open_github", { id: detail.definition.id })
                  }
                >
                  <ExternalLink size={14} />
                  GitHub 릴리스
                </button>
                <button
                  disabled={!detail.installed}
                  onClick={() =>
                    void action("open_location", { id: detail.definition.id })
                  }
                >
                  <FolderOpen size={14} />
                  설치 폴더
                </button>
                {!detail.isLauncher && (
                  <button
                    disabled={busy}
                    onClick={() => edit(detail.definition)}
                  >
                    프로그램 설정
                  </button>
                )}
              </div>
            </div>
            <div className="modal-footer">
              <button onClick={() => setSelected(null)}>닫기</button>
              {detail.installed && !detail.isLauncher && (
                <button
                  className="primary"
                  disabled={detail.busy}
                  onClick={() =>
                    void action("launch_app", { id: detail.definition.id })
                  }
                >
                  앱 실행
                  <ArrowUpRight size={14} />
                </button>
              )}
            </div>
          </section>
        </div>
      )}
      {editor && (
        <Editor
          error={error}
          initial={editor}
          existing={editingExisting}
          busy={busy}
          close={() => setEditor(null)}
          save={async (definition) => {
            setPending(true);
            setError("");
            try {
              await invoke("save_program", { definition });
              setEditor(null);
              navigate("all");
              setSearch("");
            } catch (e) {
              setError(errorText(e));
            } finally {
              setPending(false);
            }
          }}
        />
      )}
      {imported && (
        <div className="modal-backdrop">
          <section
            className="modal import-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="카탈로그 가져오기 확인"
          >
            <header className="modal-header">
              <div>
                <h2>프로그램 {imported.apps.length}개 가져오기</h2>
                <p>같은 ID의 도구는 프로그램 설정과 배포 출처가 변경됩니다.</p>
              </div>
              <button onClick={() => setImported(null)}>
                <X size={18} />
              </button>
            </header>
            <div className="modal-scroll">
              {error && <div className="global-error" role="alert">{error}</div>}
              {imported.apps.map((definition) => {
                const previous = snapshot.apps.find(
                  (app) => app.definition.id === definition.id,
                )?.definition;
                return (
                  <div className="import-item" key={definition.id}>
                    <strong>{definition.name}</strong>
                    <span>
                      {definition.repository}
                      {definition.isPrivate ? " · 비공개" : ""}
                    </span>
                    {previous && (
                      <small className="warning">
                        기존 설정 변경
                        {previous.repository !== definition.repository
                          ? ` · ${previous.repository} → ${definition.repository}`
                          : ""}
                      </small>
                    )}
                  </div>
                );
              })}
            </div>
            <div className="modal-footer">
              <button onClick={() => setImported(null)}>취소</button>
              <button
                className="primary"
                disabled={busy}
                onClick={async () => {
                  setPending(true);
                  try {
                    await invoke("confirm_import", { catalog: imported });
                    setImported(null);
                  } catch (e) {
                    setError(errorText(e));
                  } finally {
                    setPending(false);
                  }
                }}
              >
                확인 후 가져오기
              </button>
            </div>
          </section>
        </div>
      )}
    </div>
  );
}
function Switch({
  label,
  note,
  checked,
  onChange,
}: {
  label: string;
  note?: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="setting-row">
      <span>
        <strong>{label}</strong>
        {note && <small>{note}</small>}
      </span>
      <button
        type="button"
        role="switch"
        aria-label={label}
        aria-checked={checked}
        className={checked ? "switch on" : "switch"}
        onClick={() => onChange(!checked)}
      >
        <i />
      </button>
    </label>
  );
}
function Editor({
  error,
  initial,
  existing,
  busy,
  close,
  save,
}: {
  error: string;
  initial: AppDefinition;
  existing: boolean;
  busy: boolean;
  close: () => void;
  save: (definition: AppDefinition) => void;
}) {
  const [draft, setDraft] = useState(initial),
    [index, setIndex] = useState(0),
    [showImages, setShowImages] = useState(false);
  const pkg = draft.packages[index];
  function field(key: keyof AppDefinition, value: unknown) {
    setDraft((current) => ({ ...current, [key]: value }));
  }
  function packageField(key: string, value: string) {
    setDraft((current) => ({
      ...current,
      packages: current.packages.map((item, i) =>
        i === index ? { ...item, [key]: value } : item,
      ),
    }));
  }
  function submit(e: React.FormEvent) {
    e.preventDefault();
    let repository = draft.repository
      .trim()
      .replace(/^https:\/\/github\.com\//i, "")
      .replace(/\/$/, "")
      .replace(/\.git$/i, "");
    save({
      ...draft,
      id: draft.id.trim(),
      name: draft.name.trim(),
      repository,
      monogram: draft.monogram.trim() || draft.name.slice(0, 2),
      packages: draft.packages.map((item) => ({
        ...item,
        assetPattern: item.assetPattern.trim(),
        executablePath: item.executablePath.trim(),
        installDirectory:
          item.kind === "zip" ? null : item.installDirectory?.trim(),
      })),
    });
  }
  return (
    <div className="modal-backdrop">
      <form
        className="modal editor"
        role="dialog"
        aria-modal="true"
        aria-label={existing ? "프로그램 설정" : "프로그램 추가"}
        onSubmit={submit}
      >
        <header className="modal-header">
          <div>
            <h2>{existing ? "프로그램 설정" : "프로그램 추가"}</h2>
            <p>GitHub Releases에서 설치할 도구를 연결하세요.</p>
          </div>
          <button type="button" aria-label="프로그램 설정 닫기" onClick={close}>
            <X size={19} />
          </button>
        </header>
        <div className="modal-scroll form-grid">
          {error && <div className="global-error wide" role="alert">{error}</div>}
          <label>
            프로그램 이름
            <input
              required
              maxLength={80}
              value={draft.name}
              onChange={(e) => field("name", e.target.value)}
            />
          </label>
          <label>
            ID
            <input
              required
              disabled={existing}
              maxLength={64}
              pattern="[a-z0-9][a-z0-9-]*"
              placeholder="my-tool"
              value={draft.id}
              onChange={(e) => field("id", e.target.value)}
            />
          </label>
          <label className="wide">
            GitHub 저장소
            <input
              required
              placeholder="owner/repository 또는 GitHub 주소"
              value={draft.repository}
              onChange={(e) => field("repository", e.target.value)}
            />
          </label>
          <label className="checkbox-label wide">
            <input
              type="checkbox"
              checked={draft.isPrivate}
              onChange={(e) => field("isPrivate", e.target.checked)}
            />
            비공개 앱 · 개인 카탈로그에만 저장
          </label>
          <label className="wide">
            설명
            <textarea
              maxLength={500}
              value={draft.description}
              onChange={(e) => field("description", e.target.value)}
            />
          </label>
          <label>
            설치 형식
            <select
              value={pkg.kind}
              onChange={(e) => packageField("kind", e.target.value)}
            >
              <option value="zip">포터블 ZIP</option>
              <option value="inno">Inno Setup 설치 파일</option>
              <option value="nsis">NSIS 설치 파일</option>
            </select>
          </label>
          <label>
            프로세서
            <select
              value={pkg.architecture}
              onChange={(e) => {
                const found = draft.packages.findIndex(
                  (item) => item.architecture === e.target.value,
                );
                if (found >= 0) setIndex(found);
                else packageField("architecture", e.target.value);
              }}
            >
              {["x64", "arm64", "x86"].map((arch) => (
                <option key={arch}>{arch}</option>
              ))}
            </select>
          </label>
          <label className="wide">
            배포 파일 패턴
            <input
              required
              placeholder="MyTool-*-Setup-x64.exe"
              value={pkg.assetPattern}
              onChange={(e) => packageField("assetPattern", e.target.value)}
            />
          </label>
          <label className="wide">
            실행 파일 상대 경로
            <input
              required
              placeholder="MyTool.exe"
              value={pkg.executablePath}
              onChange={(e) => packageField("executablePath", e.target.value)}
            />
          </label>
          {pkg.kind !== "zip" && (
            <label className="wide">
              설치 폴더 · %LOCALAPPDATA% 기준
              <input
                required
                placeholder="Programs/MyTool"
                value={pkg.installDirectory || ""}
                onChange={(e) =>
                  packageField("installDirectory", e.target.value)
                }
              />
            </label>
          )}
          <button
            className="disclosure wide"
            type="button"
            onClick={() => setShowImages(!showImages)}
          >
            아이콘·커버 설정
            <ChevronRight
              size={14}
              style={{ transform: showImages ? "rotate(90deg)" : "none" }}
            />
          </button>
          {showImages && (
            <>
              <label className="wide">
                아이콘 URL
                <input
                  placeholder="같은 GitHub 저장소의 raw PNG/JPG 주소"
                  value={draft.iconUrl || ""}
                  onChange={(e) => field("iconUrl", e.target.value || null)}
                />
              </label>
              <label className="wide">
                커버 URL
                <input
                  value={draft.coverUrl || ""}
                  onChange={(e) => field("coverUrl", e.target.value || null)}
                />
              </label>
            </>
          )}
        </div>
        <div className="modal-footer">
          <button type="button" onClick={close}>
            취소
          </button>
          <button className="primary" disabled={busy} type="submit">
            <Check size={14} />
            저장
          </button>
        </div>
      </form>
    </div>
  );
}
export default App;
