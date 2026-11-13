"use client";
// YASB Taskbar page: the real desktop on the left with a mock bar drawn over it from the config itself (changes show immediately),
// and 5 control tabs on the right. When the user has hand-edited the YAML/CSS files, a notice strip is shown and the user is asked before the controls change anything.
import { useCallback, useEffect, useState } from "react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { DesktopPreview } from "@/components/DesktopPreview";
import { IconButton } from "@/components/Group";
import { SubTabs, TabPanel } from "@/components/SubTabs";
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import { normalizeTaskbarConfig, type TaskbarConfig } from "@/lib/taskbar";
import type { TaskbarRawState, TaskbarStatus } from "@/lib/types";
import { useModuleConfig } from "@/lib/useModuleConfig";
import type { ModuleBodyProps } from "./index";
import { BarPreview } from "./taskbar/BarPreview";
import { BehaviorTab } from "./taskbar/BehaviorTab";
import { ColorsTab } from "./taskbar/ColorsTab";
import { PositionTab } from "./taskbar/PositionTab";
import { TextTab } from "./taskbar/TextTab";
import { WidgetsTab } from "./taskbar/WidgetsTab";

type Tab = "position" | "colors" | "text" | "widgets" | "behavior";
const TABS: Tab[] = ["position", "colors", "text", "widgets", "behavior"];
const KEEP_KEY = "sg.taskbar.keepUserFiles";

function readKept(): boolean {
  try {
    return localStorage.getItem(KEEP_KEY) === "1";
  } catch {
    return false;
  }
}
function writeKept(on: boolean) {
  try {
    if (on) localStorage.setItem(KEEP_KEY, "1");
    else localStorage.removeItem(KEEP_KEY);
  } catch {
    // It's fine if the choice can't be saved: we simply ask again next time.
  }
}

type Asking = "overwrite" | "import" | null;

export function TaskbarView({ info }: ModuleBodyProps) {
  const { t, tt } = useT();
  const { toast } = useApp();
  const { cfg, update, flush } = useModuleConfig<TaskbarConfig>("taskbar", normalizeTaskbarConfig);
  const [tab, setTab] = useState<Tab>("position");
  const [status, setStatus] = useState<TaskbarStatus | null>(null);
  const [raw, setRaw] = useState<TaskbarRawState | null>(null);
  const [kept, setKept] = useState(false);
  const [pending, setPending] = useState<Partial<TaskbarConfig> | null>(null);
  const [asking, setAsking] = useState<Asking>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => setKept(readKept()), []);

  const refresh = useCallback(() => {
    api.taskbarStatus().then(setStatus).catch(() => {});
    api.taskbarRawState().then(setRaw).catch(() => {});
  }, []);

  useEffect(() => {
    refresh();
    const offStatus = api.onTaskbarStatus((s) => {
      setStatus(s);
      setRaw(s.rawState);
    });
    const offCfg = api.onModuleConfigChanged((id) => id === "taskbar" && refresh());
    return () => {
      offStatus.then((f) => f());
      offCfg.then((f) => f());
    };
  }, [info.enabled, refresh]);

  // The core manages the files itself again (the app's controls were used) → forget the "keep my files" choice. Only once the real state
  // has loaded (`raw` starts as null); otherwise every page open would clear the choice before we could know whether the files were hand-edited.
  useEffect(() => {
    if (raw === "generated") {
      setKept(false);
      writeKept(false);
    }
  }, [raw]);

  const fail = (e: unknown) => toast("err", api.errorText(e, tt));

  const set = (patch: Partial<TaskbarConfig>) => {
    if (raw === "edited") setPending(patch);
    else update(patch);
  };

  /** The user agreed to the change while using hand-edited files: write the change, then let the app take control back. */
  const confirmChange = async () => {
    if (!pending) return;
    setBusy(true);
    try {
      update(pending);
      setPending(null);
      await flush();
      await api.taskbarRawChoose(false);
      refresh();
    } catch (e) {
      fail(e);
    }
    setBusy(false);
  };

  const reuseApp = async () => {
    setAsking(null);
    setBusy(true);
    try {
      await flush();
      await api.taskbarRawChoose(false);
      refresh();
    } catch (e) {
      fail(e);
    }
    setBusy(false);
  };

  const keepMine = async () => {
    try {
      await api.taskbarRawChoose(true);
      setKept(true);
      writeKept(true);
      refresh();
    } catch (e) {
      fail(e);
    }
  };

  const importTheme = async () => {
    setAsking(null);
    try {
      const dir = await api.pickFolder();
      if (!dir) return;
      await api.taskbarImportYasb(dir);
      toast("ok", t("taskbar.import.done"));
      refresh();
    } catch (e) {
      fail(e);
    }
  };

  const edited = raw === "edited";
  return (
    <div className="cur-body lb-body gl-body tb-body">
      <div className="notice a lb-state" style={{ ["--d" as string]: ".2s" }}>
        <b>{info.enabled ? t("taskbar.strip.on") : t("taskbar.strip.off")}</b>
        {status?.nativeHidden && <span>{t("taskbar.strip.native_hidden")}</span>}
        {status?.error && <span>{tt(status.error)}</span>}
        <span className="tb-links">
          <IconButton label={t("taskbar.import.button")} onClick={() => (edited ? setAsking("import") : void importTheme())} />
          <IconButton label={t("taskbar.raw.open")} onClick={() => api.taskbarOpenRaw().catch(fail)} />
        </span>
      </div>
      {edited && (
        <div className="notice a lb-state fallback" style={{ ["--d" as string]: ".24s" }}>
          <b>{t("taskbar.raw.edited")}</b>
          <span>{kept ? t("taskbar.raw.kept") : t("taskbar.raw.edited_hint")}</span>
          {!kept && (
            <span className="tb-links">
              <IconButton label={t("taskbar.raw.use_app")} disabled={busy} onClick={() => setAsking("overwrite")} />
              <IconButton label={t("taskbar.raw.keep")} disabled={busy} onClick={() => void keepMine()} />
            </span>
          )}
        </div>
      )}

      {cfg && (
        <div className="lb-main a tb-main" style={{ ["--d" as string]: ".28s" }}>
          <div className="lb-pv">
            <DesktopPreview>
              <BarPreview cfg={cfg} />
            </DesktopPreview>
            <small>{t("taskbar.preview.hint")}</small>
          </div>
          <div className="lb-side">
            <SubTabs<Tab> tabs={TABS.map((id) => ({ id, label: t(`taskbar.tab.${id}`) }))} value={tab} onChange={setTab} />
            <TabPanel tab={tab}>
              {tab === "position" && <PositionTab cfg={cfg} set={set} />}
              {tab === "colors" && <ColorsTab cfg={cfg} set={set} />}
              {tab === "text" && <TextTab cfg={cfg} set={set} />}
              {tab === "widgets" && <WidgetsTab cfg={cfg} set={set} />}
              {tab === "behavior" && <BehaviorTab cfg={cfg} set={set} />}
            </TabPanel>
          </div>
        </div>
      )}

      {pending && (
        <ConfirmDialog
          title={t("taskbar.raw.change.title")}
          message={t("taskbar.raw.change.body")}
          confirmLabel={t("taskbar.raw.change.confirm")}
          danger
          busy={busy}
          onCancel={() => setPending(null)}
          onConfirm={() => void confirmChange()}
        />
      )}
      {asking === "overwrite" && (
        <ConfirmDialog
          title={t("taskbar.raw.overwrite.title")}
          message={t("taskbar.raw.overwrite.body")}
          confirmLabel={t("taskbar.raw.overwrite.confirm")}
          danger
          busy={busy}
          onCancel={() => setAsking(null)}
          onConfirm={() => void reuseApp()}
        />
      )}
      {asking === "import" && (
        <ConfirmDialog
          title={t("taskbar.import.button")}
          message={t("taskbar.import.confirm.body")}
          confirmLabel={t("taskbar.import.confirm.ok")}
          danger
          onCancel={() => setAsking(null)}
          onConfirm={() => void importTheme()}
        />
      )}
    </div>
  );
}
