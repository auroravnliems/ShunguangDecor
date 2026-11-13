// Sample data for running the UI in a browser (no Tauri). Only for viewing and screenshotting the UI.
import { ROLES, type SchemeDetail } from "./cursor";
import { MODULE_IDS, type ModuleInfo, type Settings } from "./types";

const settings: Settings = { language: "vi", autostart: true, reduceMotion: false, followOsMotion: false };
const modules: ModuleInfo[] = MODULE_IDS.map((id, i) => ({
  id,
  number: i + 1,
  availability: { kind: "unsupported", reason: { key: "reason.coming_soon", args: {} } },
  enabled: false,
  fault: null,
  status: { summary: null, details: [] },
}));

function mockRoles(animated: boolean): SchemeDetail["roles"] {
  const roles: SchemeDetail["roles"] = {};
  for (const r of ROLES) {
    const frames = animated && (r === "Wait" || r === "AppStarting") ? 6 : 1;
    roles[r] = { file: `${r}.cur`, frames, delaysMs: Array(frames).fill(100), width: 32, height: 32, hotspot: [2, 2] };
  }
  return roles;
}
const MOCK_SCHEMES: SchemeDetail[] = [
  { id: "mau-mot-000001", name: "Bộ mẫu một", importedUnix: 1, roles: mockRoles(true), files: ROLES.map((r) => `${r}.cur`) },
  { id: "mau-hai-000002", name: "Bộ mẫu hai", importedUnix: 2, roles: mockRoles(false), files: ROLES.map((r) => `${r}.cur`) },
];

const MOCK_WIDGETS = [
  "windows_button", "search", "apps", "tray", "clock", "volume", "network",
  "battery", "notifications", "cpu", "memory", "active_window", "power_menu",
];

/** A sample feature for the Hidden Features page when running in a browser (no Tauri). */
const MOCK_FEATURE = {
  id: 26008830,
  name: "Feature_Example",
  runtime: { id: 26008830, priority: "user", state: "enabled", variant: 0, variant_payload_kind: "none", variant_payload: 0, wexp: false },
  boot: null,
  changedByApp: false,
};

/** Fake state of the Hidden Features part: admin rights + the features that were changed (id → { runtime, boot }). */
const mockFeatures = { elevated: false, changed: new Map<number, { runtime: string | null; boot: string | null }>() };
const MOCK_SUGGESTION_IDS = [48822452, 29532725, 29785184];

function mockFeatureModule(): ModuleInfo {
  const m = modules.find((x) => x.id === "features")!;
  m.availability = { kind: "available" };
  m.status = {
    summary: null,
    details: [
      [{ key: "features.status.changed" }, { key: "features.val.n_features", args: { n: String(mockFeatures.changed.size) } }],
      [{ key: "features.status.admin" }, { key: mockFeatures.elevated ? "features.val.yes" : "features.val.not_yet" }],
      [{ key: "features.status.windows" }, { key: "features.val.build", args: { n: "26100" } }],
    ],
  };
  return m;
}
mockFeatureModule();

function mockRow(id: number) {
  const c = mockFeatures.changed.get(id);
  const cfg = (state: string | null) =>
    state ? { id, priority: "user", state, variant: 0, variant_payload_kind: "none", variant_payload: 0, wexp: false } : null;
  return { id, name: id === MOCK_FEATURE.id ? MOCK_FEATURE.name : null, runtime: cfg(c?.runtime ?? null), boot: cfg(c?.boot ?? null), changedByApp: !!c };
}
function mockSet(id: number, store: string, state: string | null) {
  const c = mockFeatures.changed.get(id) ?? { runtime: null, boot: null };
  c[store as "runtime" | "boot"] = state;
  mockFeatures.changed.set(id, c);
}

function wait<T>(v: T): Promise<T> {
  return new Promise((r) => setTimeout(() => r(v), 40));
}

export function mockInvoke(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
  switch (cmd) {
    case "get_app_info":
      return wait({
        version: "0.1.0",
        protocolBase: "",
        portable: false,
        githubUrl: null,
        discordUrl: "https://discord.gg/k55b6F3h2z",
      });
    case "get_settings":
      return wait({ ...settings });
    case "set_language":
      settings.language = String(args?.lang ?? "en");
      return wait({ ...settings });
    case "set_autostart":
      settings.autostart = Boolean(args?.enabled);
      return wait({ ...settings });
    case "set_reduce_motion":
      settings.reduceMotion = Boolean(args?.enabled);
      return wait({ ...settings });
    case "set_follow_os_motion":
      settings.followOsMotion = Boolean(args?.enabled);
      return wait({ ...settings });
    case "list_modules":
      mockFeatureModule();
      return wait(modules.map((m) => ({ ...m })));
    case "set_module_enabled": {
      const m = modules.find((x) => x.id === args?.id);
      if (m?.id !== "features") return Promise.reject({ key: "reason.coming_soon", args: {} });
      m.enabled = Boolean(args?.enabled);
      return wait({ ...mockFeatureModule() });
    }
    case "module_get_config":
      return wait({});
    case "module_set_config":
      return wait(undefined);
    case "get_runtime_stats":
      return wait({ ramBytes: 38 * 1024 * 1024 });
    case "get_current_wallpaper":
    case "take_pending_intent":
      return wait(null);
    case "list_monitors":
      return wait([{ number: 1, width: 2560, height: 1440, primary: true }]);
    case "capture_start":
    case "capture_stop":
      return wait(undefined);
    case "capture_live_source":
      return wait(null);
    case "wallpaper_library":
      return wait([
        { id: "mau-a", title: "Biển đêm", kind: "video", source: "C:\mau\bien.mp4", sourceExists: true, hasProperties: false, thumbUrl: null },
        { id: "mau-b", title: "Rừng sương", kind: "picture", source: "C:\mau\rung.png", sourceExists: true, hasProperties: false, thumbUrl: null },
        { id: "mau-c", title: "Hạt sáng", kind: "web", source: "C:\mau\hat\index.html", sourceExists: true, hasProperties: true, thumbUrl: null },
      ]);
    case "wallpaper_state":
      return wait({ running: [], manualPause: false, error: null, errors: {} });
    case "wallpaper_properties":
      return wait(null);
    case "wallpaper_find_mpv":
      return wait(null);
    case "wallpaper_toggle_pause":
      return wait(true);
    case "wallpaper_add":
    case "wallpaper_import":
    case "wallpaper_delete":
    case "wallpaper_rename":
    case "wallpaper_set_property":
    case "wallpaper_reset_properties":
    case "wallpaper_debug_taskbar_created":
      return wait(undefined);
    case "labels_status":
      return wait({
        method: "off",
        mode: "auto",
        explorerPid: null,
        items: 0,
        error: null,
        needsExplorerRestart: false,
        source: "none",
      });
    case "explorer_restart":
    case "labels_debug_taskbar_created":
      return wait(undefined);
    case "taskbar_widget_catalog":
      return wait(MOCK_WIDGETS.map((kind) => ({ kind, nameKey: `taskbar.widget.${kind}.name`, descKey: `taskbar.widget.${kind}.desc`, singleton: true })));
    case "taskbar_status":
      return wait({ enabled: false, nativeHidden: false, rawState: "generated", error: null });
    case "taskbar_raw_state":
      return wait("generated");
    case "taskbar_list_fonts":
      return wait(["Arial", "Calibri", "Consolas", "Segoe UI", "Segoe UI Variable"]);
    case "taskbar_open_raw":
    case "taskbar_raw_choose":
    case "taskbar_import_yasb":
    case "taskbar_debug_taskbar_created":
      return wait(undefined);
    case "features_search":
      return wait([MOCK_FEATURE, ...MOCK_SUGGESTION_IDS.map(mockRow)].filter((r) => !args?.query || String(r.id).includes(String(args.query)) || r.name?.toLowerCase().includes(String(args.query).toLowerCase())));
    case "features_list": {
      // Sample set: a named feature + a few config-only IDs + 120 unnamed IDs, to exercise pagination.
      const q = args?.query as { page: number; pageSize: number; configuredOnly: boolean; hideUnnamed: boolean; text: string; sort: string };
      const ids = [MOCK_FEATURE.id, ...MOCK_SUGGESTION_IDS, ...Array.from({ length: 120 }, (_, i) => 1000 + i * 7)];
      const text = q.text.trim().toLowerCase();
      let rows = ids.map(mockRow).filter((r) => {
        if (q.configuredOnly && !r.runtime && !r.boot) return false;
        if (q.hideUnnamed && !r.name) return false;
        if (!text) return true;
        return /^\d+$/.test(text) ? String(r.id).startsWith(text) : !!r.name?.toLowerCase().includes(text);
      });
      rows = rows.sort((a, b) => (q.sort === "id" ? a.id - b.id : a.name && b.name ? a.name.localeCompare(b.name) : a.name ? -1 : b.name ? 1 : a.id - b.id));
      const pageSize = Math.min(200, Math.max(10, q.pageSize));
      const page = Math.min(q.page, Math.max(0, Math.ceil(rows.length / pageSize) - 1));
      return wait({ total: rows.length, page, pageSize, rows: rows.slice(page * pageSize, (page + 1) * pageSize) });
    }
    case "features_web_search":
      return wait(undefined);
    case "features_get":
      return wait(mockRow(Number(args?.id ?? MOCK_FEATURE.id)));
    case "features_suggestions":
      return wait(MOCK_SUGGESTION_IDS.map((id) => ({ id, nameKey: `features.suggest.${id}.name`, descKey: `features.suggest.${id}.desc`, row: mockRow(id) })));
    case "features_subscriptions":
      return wait([{ id: 26008830, reporting_kind: 1, reporting_options: 0, reporting_target: "0x0F890D2BA3BC48B9" }]);
    case "features_undo_list":
      return wait([...mockFeatures.changed.keys()].map((id) => ({ unix: 1, store: "runtime", original: null, id, priority: "user" })));
    case "features_export":
      return wait(String(args?.dest ?? "features") + ".json");
    case "features_elevate":
      mockFeatures.elevated = true;
      return wait(true);
    case "features_import":
      return wait({ applied: 0, skipped: 0, warnings: [] });
    case "features_undo_all": {
      const n = mockFeatures.changed.size;
      mockFeatures.changed.clear();
      return wait(n);
    }
    case "features_fix_priority":
      return wait(0);
    case "features_notify_usage":
      return wait(undefined);
    case "features_set":
      mockFeatures.elevated = true;
      mockSet(Number(args?.id), String(args?.store), String(args?.state));
      return wait(undefined);
    case "features_reset":
      mockSet(Number(args?.id), String(args?.store), null);
      return wait(undefined);
    case "features_release":
      return wait(undefined);
    case "profile_list":
      return wait({ profiles: [], active: null });
    case "cursor_list":
      return wait(MOCK_SCHEMES.map(({ id, name, roles }) => ({ id, name, roleCount: Object.keys(roles).length, importedUnix: 1 })));
    case "cursor_detail":
      return wait(MOCK_SCHEMES.find((s) => s.id === args?.id) ?? null);
    case "cursor_import":
      return wait(["mau-000001"]);
    case "cursor_rename":
    case "cursor_delete":
    case "cursor_set_role":
      return wait(undefined);
    case "restore_all_os_defaults":
      return wait(modules.map((m) => ({ id: m.id, error: null })));
    case "import_all":
      return wait({ files: 0 });
    default:
      return wait(null);
  }
}
