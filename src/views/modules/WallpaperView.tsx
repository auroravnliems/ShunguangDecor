"use client";
// Wallpaper page: library, displays (real desktop), playback, auto-pause, Lively customization.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { IconButton } from "@/components/Group";
import { JoinButton } from "@/components/JoinButton";
import { PromptDialog } from "@/components/PromptDialog";
import { SubTabs, TabPanel } from "@/components/SubTabs";
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import type { MonitorView } from "@/lib/types";
import { useModuleConfig } from "@/lib/useModuleConfig";
import {
  EMPTY_ENGINE_STATE,
  type EngineState,
  normalizeWallpaperConfig,
  parseSourceMissing,
  runningOn,
  type WallpaperConfig,
  type WallpaperItem,
  withWallpaper,
} from "@/lib/wallpaper";
import type { ModuleBodyProps } from "./index";
import type { WallpaperCtx } from "./wallpaper/ctx";
import { LibraryTab } from "./wallpaper/LibraryTab";
import { PauseTab } from "./wallpaper/PauseTab";
import { PlayTab } from "./wallpaper/PlayTab";
import { PropsTab } from "./wallpaper/PropsTab";
import { ScreenTab } from "./wallpaper/ScreenTab";

type Tab = "library" | "screen" | "play" | "pause" | "props";
type Ask = { kind: "rename"; item: WallpaperItem } | { kind: "delete"; item: WallpaperItem } | null;

export function WallpaperView({ info }: ModuleBodyProps) {
  const { t, tt } = useT();
  const { toast, applyModule } = useApp();
  const { cfg, update, flush } = useModuleConfig<WallpaperConfig>("wallpaper", normalizeWallpaperConfig);
  const [tab, setTab] = useState<Tab>("library");
  const [items, setItems] = useState<WallpaperItem[]>([]);
  const [state, setState] = useState<EngineState>(EMPTY_ENGINE_STATE);
  const [monitors, setMonitors] = useState<MonitorView[]>([]);
  const [monitor, setMonitor] = useState(1);
  const [busy, setBusy] = useState(false);
  const [dropping, setDropping] = useState(false);
  const [ask, setAsk] = useState<Ask>(null);
  const [propsId, setPropsId] = useState<string | null>(null);
  const fail = useCallback((e: unknown) => toast("err", api.errorText(e, tt)), [toast, tt]);

  // --- data ---
  const reload = useCallback(() => {
    api.wallpaperLibrary().then(setItems).catch(fail);
  }, [fail]);
  useEffect(() => {
    reload();
    const off = api.onWallpaperLibraryChanged(reload);
    return () => {
      off.then((f) => f());
    };
  }, [reload]);

  useEffect(() => {
    api.wallpaperState().then(setState).catch(() => {});
    const off = api.onWallpaperState(setState);
    return () => {
      off.then((f) => f());
    };
  }, []);

  useEffect(() => {
    api
      .listMonitors()
      .then((l) => {
        setMonitors(l);
        setMonitor((cur) => (l.some((m) => m.number === cur) ? cur : (l.find((m) => m.primary)?.number ?? l[0]?.number ?? 1)));
      })
      .catch(() => {});
  }, []);

  // Errors from the engine: report immediately (the source_missing code is translated).
  useEffect(() => {
    const off = api.onWallpaperError((e) => {
      const path = parseSourceMissing(e.message);
      toast("err", path !== null ? t("wallpaper.err.source_missing", { path }) : t("wallpaper.notice.error", { message: e.message }));
    });
    return () => {
      off.then((f) => f());
    };
  }, [t, toast]);

  // --- add / import ---
  const added = useCallback(
    (list: WallpaperItem[], toastKey: "added" | "imported") => {
      if (list.length === 0) return;
      toast("ok", toastKey === "added" ? t("wallpaper.lib.added", { name: list[0].title }) : t("wallpaper.lib.imported", { n: list.length }));
      reload();
    },
    [reload, t, toast],
  );
  const addPaths = useCallback(
    async (paths: string[]) => {
      setBusy(true);
      for (const p of paths) {
        try {
          added([await api.wallpaperAdd(p)], "added");
        } catch (e) {
          toast("err", `${p.split(/[\\/]/).pop() ?? p}: ${api.errorText(e, tt)}`);
        }
      }
      setBusy(false);
    },
    [added, toast, tt],
  );
  const dropRef = useRef(addPaths);
  dropRef.current = addPaths;
  useEffect(() => {
    const off = api.onFileDrop(
      (paths) => void dropRef.current(paths),
      (over) => setDropping(over),
    );
    return () => {
      off.then((f) => f());
    };
  }, []);

  const addFile = async () => {
    const p = await api.pickWallpaperFile(t("wallpaper.lib.add_file"));
    if (p) await addPaths([p]);
  };
  const addUrl = async (url: string) => {
    setBusy(true);
    try {
      added([await api.wallpaperAdd(url)], "added");
    } catch (e) {
      fail(e);
    }
    setBusy(false);
  };
  const importPath = async (p: string | null) => {
    if (!p) return;
    setBusy(true);
    try {
      added(await api.wallpaperImport(p), "imported");
    } catch (e) {
      fail(e);
    }
    setBusy(false);
  };

  // --- set wallpaper ---
  const setOn = async (item: WallpaperItem) => {
    if (!cfg) return;
    update({ layout: withWallpaper(cfg, monitor, item.id) });
    if (!info.enabled) {
      await flush();
      try {
        applyModule(await api.setModuleEnabled("wallpaper", true));
      } catch (e) {
        fail(e);
      }
    }
  };

  const ctx: WallpaperCtx | null = cfg ? { info, cfg, update, state, items, monitors, monitor, setMonitor, fail } : null;

  // Customize tab: the selected wallpaper, or the running one if it has customization.
  const propsItem = useMemo(() => {
    const byId = (id: string | null | undefined) => items.find((i) => i.id === id && i.hasProperties);
    const running = runningOn(state, monitor)?.wallpaperId;
    return byId(propsId) ?? byId(running);
  }, [items, propsId, state, monitor]);

  const tabs = useMemo(
    () => [
      { id: "library" as const, label: t("wallpaper.tab.library") },
      { id: "screen" as const, label: t("wallpaper.tab.screen") },
      { id: "play" as const, label: t("wallpaper.tab.play") },
      { id: "pause" as const, label: t("wallpaper.tab.pause") },
      ...(propsItem ? [{ id: "props" as const, label: t("wallpaper.tab.props") }] : []),
    ],
    [t, propsItem],
  );
  // If the Customize tab disappears (another wallpaper has no customization), go back to Library.
  useEffect(() => {
    if (tab === "props" && !propsItem) setTab("library");
  }, [tab, propsItem]);

  return (
    <div className="cur-body wp-body">
      <div className="cur-top a" style={{ ["--d" as string]: ".2s" }}>
        <SubTabs tabs={tabs} value={tab} onChange={setTab} />
        <div className="cur-actions">
          <IconButton
            label={state.manualPause ? t("wallpaper.play.resume") : t("wallpaper.play.pause")}
            onClick={() => void api.wallpaperTogglePause().catch(fail)}
            disabled={!info.enabled}
          />
          <JoinButton label={t("wallpaper.lib.add_file")} sub="ADD" size="small" disabled={busy} onClick={() => void addFile()} />
        </div>
      </div>

      {!info.enabled && (
        <div className="notice a wp-notice" style={{ ["--d" as string]: ".25s" }}>
          <b>{t("wallpaper.notice.off")}</b>
        </div>
      )}

      <div className={`cur-main wp-main${info.enabled ? "" : " off"}`}>
        {ctx && (
          <TabPanel tab={tab}>
            {tab === "library" && (
              <LibraryTab
                ctx={ctx}
                busy={busy}
                onSet={(i) => void setOn(i)}
                onAddFile={() => void addFile()}
                onAddUrl={(u) => void addUrl(u)}
                onImportZip={() => void api.pickZipFile(t("wallpaper.lib.import_zip")).then(importPath)}
                onImportFolder={() => void api.pickFolder().then(importPath)}
                onRename={(i) => setAsk({ kind: "rename", item: i })}
                onDelete={(i) => setAsk({ kind: "delete", item: i })}
                onCustomize={(i) => {
                  setPropsId(i.id);
                  setTab("props");
                }}
              />
            )}
            {tab === "screen" && <ScreenTab ctx={ctx} />}
            {tab === "play" && <PlayTab ctx={ctx} />}
            {tab === "pause" && <PauseTab ctx={ctx} />}
            {tab === "props" && <PropsTab ctx={ctx} item={propsItem} />}
          </TabPanel>
        )}
      </div>

      {dropping && <div className="cur-drop">{t("wallpaper.lib.drop")}</div>}

      {ask?.kind === "rename" && (
        <PromptDialog
          title={t("wallpaper.lib.rename_title")}
          label={t("wallpaper.lib.rename_label")}
          initial={ask.item.title}
          maxLength={120}
          onCancel={() => setAsk(null)}
          onConfirm={(name) => {
            const id = ask.item.id;
            setAsk(null);
            api.wallpaperRename(id, name).catch(fail);
          }}
        />
      )}
      {ask?.kind === "delete" && (
        <ConfirmDialog
          title={t("wallpaper.lib.delete_title")}
          message={t("wallpaper.lib.delete_body", { name: ask.item.title })}
          confirmLabel={t("wallpaper.lib.delete")}
          danger
          onCancel={() => setAsk(null)}
          onConfirm={() => {
            const id = ask.item.id;
            setAsk(null);
            api.wallpaperDelete(id).catch(fail);
          }}
        />
      )}
    </div>
  );
}
