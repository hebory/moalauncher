import { describe, it, expect } from "vitest";
import { filterApps, hasUpdate, updateOrder } from "./library";
import type { AppView } from "./types";
function fixture(id: string, isLauncher = false): AppView {
  return {
    definition: {
      id,
      name: id,
      repository: "owner/tool",
      description: "도구",
      category: "유틸리티",
      monogram: "M",
      accentColor: "#777777",
      isPrivate: false,
      packages: [],
    },
    installed: {
      appId: id,
      version: "1.0.0",
      executablePath: "Tool.exe",
      installDirectory: "Programs/Tool",
      packageKind: "zip",
      installedAt: "",
    },
    release: {
      tag: "v2.0.0",
      name: "2",
      notes: "",
      pageUrl: "",
      publishedAt: "",
      assets: [],
      isPrivate: false,
    },
    isCached: false,
    isLauncher,
    busy: false,
    canCancel: false,
    error: null,
    progress: null,
  };
}
describe("library", () => {
  it("combines favorites and search without including other apps", () => {
    const apps = [fixture("first"), fixture("second")];
    expect(filterApps(apps, "favorites", "second", ["first"])).toHaveLength(0);
    expect(
      filterApps(apps, "favorites", "FIRST", ["first"])[0].definition.id,
    ).toBe("first");
  });
  it("keeps launcher last regardless of grid/filter display order", () =>
    expect(updateOrder([fixture("launcher", true), fixture("tool")])).toEqual([
      "tool",
      "launcher",
    ]));
  it("requires an actual newer version", () => {
    const app = fixture("tool");
    app.release!.tag = "v1.0.0";
    expect(hasUpdate(app)).toBe(false);
    app.installed = null;
    expect(hasUpdate(app)).toBe(false);
  });
  it("prioritizes notified apps only in updates", () => {
    const apps = [fixture("first"), fixture("second")];
    expect(
      filterApps(apps, "updates", "", [], ["second"])[0].definition.id,
    ).toBe("second");
    expect(filterApps(apps, "all", "", [], ["second"])[0].definition.id).toBe(
      "first",
    );
  });
});
