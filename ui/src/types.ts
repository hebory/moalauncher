export type PackageDefinition = {
  architecture: string;
  kind: string;
  assetPattern: string;
  executablePath: string;
  installDirectory?: string | null;
};
export type AppDefinition = {
  id: string;
  name: string;
  description: string;
  category: string;
  accentColor: string;
  monogram: string;
  repository: string;
  isPrivate: boolean;
  iconUrl?: string | null;
  coverUrl?: string | null;
  packages: PackageDefinition[];
};
export type InstalledApp = {
  appId: string;
  version: string;
  executablePath: string;
  installDirectory: string;
  packageKind: string;
  installedAt: string;
};
export type Release = {
  tag: string;
  name: string;
  notes: string;
  pageUrl: string;
  publishedAt: string;
  assets: unknown[];
  isPrivate: boolean;
};
export type Settings = {
  checkOnStartup: boolean;
  periodicChecks: boolean;
  updateNotifications: boolean;
  useGridView: boolean;
  favoriteAppIds: string[];
  checkIntervalMinutes: number;
  [key: string]: unknown;
};
export type AppView = {
  definition: AppDefinition;
  installed: InstalledApp | null;
  release: Release | null;
  isCached: boolean;
  isLauncher: boolean;
  busy: boolean;
  canCancel: boolean;
  error: string | null;
  progress: {
    appId: string;
    stage: string;
    bytesReceived: number;
    totalBytes: number | null;
  } | null;
};
export type Snapshot = {
  apps: AppView[];
  settings: Settings;
  githubConnected: boolean;
  startupEnabled: boolean;
  dataDirectory: string;
  version: string;
  checking: boolean;
  batchUpdating: boolean;
  status: string;
  lastChecked: string | null;
  notificationIds: string[];
  authGeneration: number;
  updateResult: {success:boolean;message:string;version:string;completedAt:string;logPath:string}|null;
};
export type Artwork = { icon: string | null; cover: string | null };
