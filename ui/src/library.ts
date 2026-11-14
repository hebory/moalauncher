import type { AppView } from "./types";
export type Filter = "all" | "favorites" | "installed" | "updates";
export function hasUpdate(app: AppView) {
  if (!app.release || !app.installed) return false;
  const parse = (value: string) => {
    const match = /^[vV]?(\d+)\.(\d+)\.(\d+)(?:-([^+]+))?/.exec(value);
    return match
      ? { numbers: match.slice(1, 4).map(Number), pre: match[4] }
      : null;
  };
  const latest = parse(app.release.tag),
    current = parse(app.installed.version);
  if (!latest || !current) return false;
  for (let i = 0; i < 3; i++) {
    if (latest.numbers[i] !== current.numbers[i])
      return latest.numbers[i] > current.numbers[i];
  }
  return !latest.pre && !!current.pre;
}
export function filterApps(
  apps: AppView[],
  filter: Filter,
  search: string,
  favorites: string[],
  notified: string[] = [],
) {
  const query = search.trim().toLocaleLowerCase();
  return apps
    .filter(
      (app) =>
        (filter !== "favorites" || favorites.includes(app.definition.id)) &&
        (filter !== "installed" || !!app.installed) &&
        (filter !== "updates" || hasUpdate(app)) &&
        `${app.definition.name} ${app.definition.description} ${app.definition.repository}`
          .toLocaleLowerCase()
          .includes(query),
    )
    .sort((a, b) =>
      filter === "updates"
        ? Number(notified.includes(b.definition.id)) -
          Number(notified.includes(a.definition.id))
        : 0,
    );
}
export function updateOrder(apps: AppView[]) {
  return apps
    .filter((app) => hasUpdate(app) && !app.error)
    .sort((a, b) => Number(a.isLauncher) - Number(b.isLauncher))
    .map((app) => app.definition.id);
}
