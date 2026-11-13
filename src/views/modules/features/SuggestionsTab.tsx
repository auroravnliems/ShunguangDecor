"use client";
// Suggestions tab: one card per verified feature. The switch writes both stores (like ViVeTool's default); "Restore default" deletes both.
import { useCallback, useEffect, useState } from "react";
import { JoinButton } from "@/components/JoinButton";
import { Toggle } from "@/components/Toggle";
import * as api from "@/lib/api";
import { DEFAULT_PRIORITY, stateLabelKey } from "@/lib/features";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import type { FeatureRow, FeatureStore, SuggestionRow } from "@/lib/types";
import type { useFeatureWrite } from "./useWrite";

const STORES: FeatureStore[] = ["runtime", "boot"];

const isOn = (row: FeatureRow) => (row.runtime ?? row.boot)?.state === "enabled";

export function SuggestionsTab(p: { rev: number; write: ReturnType<typeof useFeatureWrite> }) {
  const { t, tt } = useT();
  const { toast } = useApp();
  const [list, setList] = useState<SuggestionRow[] | null>(null);
  const { run, busy } = p.write;

  const load = useCallback(
    () =>
      api
        .featuresSuggestions()
        .then(setList)
        .catch((e) => {
          setList([]);
          toast("err", api.errorText(e, tt));
        }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  useEffect(() => {
    void load();
  }, [load, p.rev]);

  const toggle = (s: SuggestionRow, on: boolean) =>
    run(async () => {
      for (const store of STORES) await api.featuresSet(s.id, store, on ? "enabled" : "disabled", 0, DEFAULT_PRIORITY);
      toast("ok", t("features.toast.applied_boot"));
    });
  const reset = (s: SuggestionRow) =>
    run(async () => {
      for (const store of STORES) await api.featuresReset(s.id, store, DEFAULT_PRIORITY);
      toast("ok", t("features.toast.reset"));
    });

  if (list === null) return null;
  if (list.length === 0) return <p className="lb-hint">{t("features.suggest.empty")}</p>;
  return (
    <div className="ft-cards">
      {list.map((s) => (
        <div key={s.id} className="ft-card">
          <div className="ft-card-top">
            <b>{t(s.nameKey)}</b>
            <Toggle on={isOn(s.row)} disabled={busy} onChange={(on) => void toggle(s, on)} label={t(s.nameKey)} />
          </div>
          <p>{t(s.descKey)}</p>
          <div className="ft-card-foot">
            <code>ID {s.id}</code>
            <span>
              {t("features.col.runtime")}: {t(stateLabelKey(s.row.runtime?.state ?? "default"))}
            </span>
            <span>
              {t("features.col.boot")}: {t(stateLabelKey(s.row.boot?.state ?? "default"))}
            </span>
            <JoinButton label={t("features.card.reset")} variant="ghost" size="small" disabled={busy} onClick={() => void reset(s)} />
          </div>
        </div>
      ))}
    </div>
  );
}
