"use client";
// Hidden features page: a permission-status strip + undo-all, then four tabs: Suggested / All / Search / Tools.
// Every write goes through useFeatureWrite (a warning the first time in a session; errors are always shown to the user).
import { useEffect, useState } from "react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { IconButton } from "@/components/Group";
import { SubTabs, TabPanel } from "@/components/SubTabs";
import * as api from "@/lib/api";
import { adminElevated, changedCount, undoTargetCount } from "@/lib/features";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import type { ModuleBodyProps } from "./index";
import { AllTab } from "./features/AllTab";
import { SearchTab } from "./features/SearchTab";
import { SuggestionsTab } from "./features/SuggestionsTab";
import { ToolsTab } from "./features/ToolsTab";
import { useFeatureWrite, useRefreshModules } from "./features/useWrite";

type Tab = "suggest" | "all" | "search" | "tools";

export function FeaturesView({ info }: ModuleBodyProps) {
  const { t, tt } = useT();
  const { toast } = useApp();
  const refresh = useRefreshModules();
  const [tab, setTab] = useState<Tab>("suggest");
  /** Incremented on every change so the tabs reload their data. */
  const [rev, setRev] = useState(0);
  const write = useFeatureWrite(() => setRev((r) => r + 1));
  const [undoCount, setUndoCount] = useState<number | null>(null);
  const [eleBusy, setEleBusy] = useState(false);

  const admin = adminElevated(info.status.details);
  const changed = changedCount(info.status.details);

  // Leaving the page closes the admin helper process (undoes nothing).
  useEffect(
    () => () => {
      api
        .featuresRelease()
        .catch((e) => api.logFrontend("warn", `features_release failed: ${String(e)}`))
        // Closing the admin process changes the permission state: re-read it so the status strip is correct when the user returns.
        .finally(() => void refresh());
    },
    [refresh],
  );

  const elevate = async () => {
    setEleBusy(true);
    try {
      const ok = await api.featuresElevate();
      write.setDenied(!ok);
      if (!ok) toast("ok", t("features.strip.denied"));
    } catch (e) {
      toast("err", api.errorText(e, tt));
    } finally {
      // Even when the user clicks "No" or an error occurs: re-read the real state from Rust.
      await refresh();
    }
    setEleBusy(false);
  };

  const askUndo = async () => {
    try {
      const list = await api.featuresUndoList();
      if (list.length === 0) toast("ok", t("features.undo.none"));
      else setUndoCount(undoTargetCount(list));
    } catch (e) {
      toast("err", api.errorText(e, tt));
    }
  };
  const undoAll = async () => {
    const n = await write.run(() => api.featuresUndoAll(), { skipWarning: true });
    setUndoCount(null);
    if (n !== undefined) toast("ok", t("features.undo.done", { n }));
  };

  return (
    <div className="ft-body">
      <div className={`notice a ft-strip${admin ? "" : " warn"}`} style={{ ["--d" as string]: ".2s" }}>
        <b>{admin ? t("features.strip.admin_yes") : t("features.strip.admin_no")}</b>
        {!admin && <IconButton label={t("features.strip.grant")} disabled={eleBusy} onClick={() => void elevate()} />}
        {!admin && write.denied && <span>{t("features.strip.denied")}</span>}
        <span>{t("features.strip.changed", { n: changed })}</span>
        <IconButton label={t("features.undo.button")} disabled={write.busy || changed === 0} onClick={() => void askUndo()} />
      </div>
      {info.status.summary && (
        <div className="notice a ft-strip warn" role="alert" style={{ ["--d" as string]: ".23s" }}>
          <span>{tt(info.status.summary)}</span>
        </div>
      )}
      <div className="ft-tabs a" style={{ ["--d" as string]: ".26s" }}>
        <SubTabs<Tab>
          tabs={[
            { id: "suggest", label: t("features.tab.suggest") },
            { id: "all", label: t("features.tab.all") },
            { id: "search", label: t("features.tab.search") },
            { id: "tools", label: t("features.tab.tools") },
          ]}
          value={tab}
          onChange={setTab}
        />
      </div>
      <div className="ft-scroll a" style={{ ["--d" as string]: ".3s" }}>
        <TabPanel tab={tab}>
          {tab === "suggest" && <SuggestionsTab rev={rev} write={write} />}
          {tab === "all" && <AllTab rev={rev} write={write} />}
          {tab === "search" && <SearchTab rev={rev} write={write} />}
          {tab === "tools" && <ToolsTab write={write} />}
        </TabPanel>
      </div>
      {write.dialog}
      {undoCount !== null && (
        <ConfirmDialog
          title={t("features.undo.title")}
          message={t("features.undo.body", { n: undoCount })}
          confirmLabel={t("features.undo.confirm")}
          danger
          busy={write.busy}
          onCancel={() => setUndoCount(null)}
          onConfirm={() => void undoAll()}
        />
      )}
    </div>
  );
}
