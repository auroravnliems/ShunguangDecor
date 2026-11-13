"use client";
// Shared UI state: app info, settings, the 6 parts, the page being viewed, notifications.
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import * as api from "./api";
import type { AppInfo, ModuleId, ModuleInfo, PageId, Settings } from "./types";

export type ToastKind = "ok" | "err";
export interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
}

interface App {
  ready: boolean;
  info: AppInfo | null;
  settings: Settings | null;
  modules: ModuleInfo[];
  page: PageId;
  navigate: (p: PageId) => void;
  applySettings: (s: Settings) => void;
  applyModule: (m: ModuleInfo) => void;
  module: (id: ModuleId) => ModuleInfo | undefined;
  toasts: Toast[];
  toast: (kind: ToastKind, text: string) => void;
  dismissToast: (id: number) => void;
  confirmRestore: boolean;
  setConfirmRestore: (on: boolean) => void;
  /** The tray asked to turn off Hidden features: the UI must ask about undoing first (intent "confirm-disable-features"). */
  confirmDisableFeatures: boolean;
  setConfirmDisableFeatures: (on: boolean) => void;
  trayNotice: boolean;
  setTrayNotice: (on: boolean) => void;
}

const Ctx = createContext<App | null>(null);

export function useApp(): App {
  const v = useContext(Ctx);
  if (!v) throw new Error("useApp() ngoài AppProvider");
  return v;
}

export function AppProvider({ children }: { children: ReactNode }) {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [modules, setModules] = useState<ModuleInfo[]>([]);
  const [page, setPage] = useState<PageId>("home");
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [confirmRestore, setConfirmRestore] = useState(false);
  const [confirmDisableFeatures, setConfirmDisableFeatures] = useState(false);
  const [trayNotice, setTrayNotice] = useState(false);
  const nextToast = useRef(1);

  const toast = useCallback((kind: ToastKind, text: string) => {
    const id = nextToast.current++;
    setToasts((ts) => [...ts.slice(-3), { id, kind, text }]);
    setTimeout(() => setToasts((ts) => ts.filter((x) => x.id !== id)), 4000);
  }, []);
  const dismissToast = useCallback((id: number) => setToasts((ts) => ts.filter((x) => x.id !== id)), []);
  const applyModule = useCallback(
    (m: ModuleInfo) => setModules((ms) => (ms.some((x) => x.id === m.id) ? ms.map((x) => (x.id === m.id ? m : x)) : [...ms, m])),
    [],
  );
  const handleIntent = useCallback((intent: string | null) => {
    if (intent === "confirm-restore") {
      setPage("settings");
      setConfirmRestore(true);
    } else if (intent === "confirm-disable-features") {
      setConfirmDisableFeatures(true);
    }
  }, []);

  useEffect(() => {
    let alive = true;
    const offs: Promise<() => void>[] = [];
    Promise.all([api.getAppInfo(), api.getSettings(), api.listModules()])
      .then(([i, s, m]) => {
        if (!alive) return;
        setInfo(i);
        setSettings(s);
        setModules(m);
        return api.takePendingIntent();
      })
      .then((intent) => alive && handleIntent(intent ?? null))
      .catch((e) => api.logFrontend("error", `startup failed: ${String(e)}`));
    offs.push(api.onModuleChanged(applyModule));
    offs.push(api.onSettingsChanged(setSettings));
    // The event is only a notification; the real work lives in a queue on the Rust side (taking an item removes it → never opened twice).
    offs.push(api.onIntent(() => api.takePendingIntent().then((i) => alive && handleIntent(i ?? null))));
    offs.push(api.onShowTrayNotice(() => setTrayNotice(true)));
    offs.push(
      api.onModuleFaulted((f) => {
        api.logFrontend("warn", `module ${f.id} faulted: ${f.message}`);
      }),
    );
    const onErr = (e: ErrorEvent) => api.logFrontend("error", `${e.message} @ ${e.filename}:${e.lineno}`);
    const onRej = (e: PromiseRejectionEvent) => api.logFrontend("error", `unhandled rejection: ${JSON.stringify(e.reason)}`);
    window.addEventListener("error", onErr);
    window.addEventListener("unhandledrejection", onRej);
    return () => {
      alive = false;
      offs.forEach((p) => p.then((off) => off()));
      window.removeEventListener("error", onErr);
      window.removeEventListener("unhandledrejection", onRej);
    };
  }, [applyModule, handleIntent]);

  // Reduced motion: a single attribute on <html>, motion.css handles the rest.
  // The Windows "turn off animations" setting only takes effect when the user has enabled "Follow Windows":
  // "PC optimizer" apps often turn off Animation effects without the user knowing.
  useEffect(() => {
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const apply = () => {
      const reduced = settings?.reduceMotion || (settings?.followOsMotion && mq.matches);
      document.documentElement.dataset.motion = reduced ? "reduced" : "full";
    };
    apply();
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [settings?.reduceMotion, settings?.followOsMotion]);

  const value = useMemo<App>(
    () => ({
      ready: info !== null && settings !== null,
      info,
      settings,
      modules,
      page,
      navigate: setPage,
      applySettings: setSettings,
      applyModule,
      module: (id) => modules.find((m) => m.id === id),
      toasts,
      toast,
      dismissToast,
      confirmRestore,
      setConfirmRestore,
      confirmDisableFeatures,
      setConfirmDisableFeatures,
      trayNotice,
      setTrayNotice,
    }),
    [info, settings, modules, page, applyModule, toasts, toast, dismissToast, confirmRestore, confirmDisableFeatures, trayNotice],
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}
