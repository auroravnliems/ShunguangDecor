"use client";
// A 1280×760 stage that scales with the window + header + decorations + pages with transition effects.
import { useEffect, useState, type ReactNode } from "react";
import * as api from "@/lib/api";
import { fitStage } from "@/lib/fit";
import { I18nProvider, useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import type { ModuleId, PageId } from "@/lib/types";
import { GalleryView } from "@/views/GalleryView";
import { HomeView } from "@/views/HomeView";
import { ModuleView } from "@/views/ModuleView";
import { useModuleToggle } from "@/views/modules/useModuleToggle";
import { SettingsView } from "@/views/SettingsView";
import { ConfirmDialog } from "./ConfirmDialog";
import { Decor } from "./Decor";
import { Header } from "./Header";
import { PageFrame } from "./PageFrame";
import { Toasts } from "./Toasts";
import { TrayNotice } from "./TrayNotice";

/** How long the old page slides up and fades before the new page appears (v9 design: 380 ms). */
const LEAVE_MS = 380;
const DEV = process.env.NODE_ENV !== "production";

function useFit() {
  const [fit, setFit] = useState({ zoom: 1, width: 1280, height: 760 });
  useEffect(() => {
    const up = () => setFit(fitStage(window.innerWidth, window.innerHeight));
    up();
    window.addEventListener("resize", up);
    return () => window.removeEventListener("resize", up);
  }, []);
  return fit;
}

function renderPage(p: PageId): ReactNode {
  switch (p) {
    case "home":
      return <HomeView />;
    case "settings":
      return <SettingsView />;
    case "gallery":
      return DEV ? <GalleryView /> : <HomeView />;
    default:
      return <ModuleView id={p as ModuleId} />;
  }
}

/** Tray "disable Hidden Features" click: runs the same undo prompt as the on-page toggle (works from any page). */
function TrayDisableGuard() {
  const { confirmDisableFeatures, setConfirmDisableFeatures, module } = useApp();
  const { request, guardNode } = useModuleToggle();
  useEffect(() => {
    if (!confirmDisableFeatures) return;
    setConfirmDisableFeatures(false);
    if (module("features")?.enabled) request("features", false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [confirmDisableFeatures]);
  return <>{guardNode}</>;
}

function RestoreConfirm() {
  const { t, tt } = useT();
  const { confirmRestore, setConfirmRestore, toast, modules, applyModule } = useApp();
  const [busy, setBusy] = useState(false);
  if (!confirmRestore) return null;
  const run = async () => {
    setBusy(true);
    try {
      const res = await api.restoreAllOsDefaults();
      const failed = res.filter((r) => r.error);
      if (failed.length === 0) toast("ok", t("toast.restore_done"));
      else failed.forEach((f) => toast("err", `${t(`module.${f.id}.name`)}: ${tt(f.error!)}`));
      const fresh = await api.listModules();
      fresh.forEach(applyModule);
    } catch (e) {
      toast("err", api.errorText(e, tt));
    } finally {
      setBusy(false);
      setConfirmRestore(false);
    }
  };
  return (
    <ConfirmDialog
      title={t("dialog.restore.title")}
      message={
        <>
          {t("dialog.restore.body", { count: modules.length })}
          <br />
          {t("dialog.restore.features_note")}
        </>
      }
      confirmLabel={t("dialog.restore.ok")}
      danger
      busy={busy}
      onConfirm={run}
      onCancel={() => setConfirmRestore(false)}
    />
  );
}

function Stage() {
  const { page, navigate } = useApp();
  const fit = useFit();
  const [shown, setShown] = useState<PageId>(page);
  const [leaving, setLeaving] = useState<PageId | null>(null);

  useEffect(() => {
    if (page === shown) return;
    setLeaving(shown);
    setShown(page);
    const tm = setTimeout(() => setLeaving(null), LEAVE_MS);
    return () => clearTimeout(tm);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page]);

  // Gallery page (dev builds only): Ctrl+Shift+G.
  useEffect(() => {
    if (!DEV) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "g") navigate("gallery");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate]);

  return (
    <div id="stage" style={{ width: fit.width, height: fit.height, zoom: fit.zoom }}>
      <Decor page={shown} />
      <Header />
      <main>
        {leaving && leaving !== shown && (
          <PageFrame key={`leave-${leaving}`} state="leave">
            {renderPage(leaving)}
          </PageFrame>
        )}
        <PageFrame key={shown} state={leaving ? "wait" : "enter"}>
          {renderPage(shown)}
        </PageFrame>
      </main>
      <Toasts />
      <TrayNotice />
      <RestoreConfirm />
      <TrayDisableGuard />
    </div>
  );
}

export function Shell() {
  const { ready, settings } = useApp();
  if (!ready || !settings) return null;
  return (
    <I18nProvider lang={settings.language}>
      <Stage />
    </I18nProvider>
  );
}
