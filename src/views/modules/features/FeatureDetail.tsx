"use client";
// Detail panel for one feature: set the state / variant / store / priority, then Apply or Reset to default.
// Shared by the Search and All tabs; every write goes through `write.run` (session warning + error reporting).
import { useEffect, useState } from "react";
import { Group, Row } from "@/components/Group";
import { JoinButton } from "@/components/JoinButton";
import { Segmented } from "@/components/Segmented";
import { SelectBox } from "@/components/SelectBox";
import * as api from "@/lib/api";
import { clampVariant, DEFAULT_PRIORITY, EDITABLE_PRIORITIES, PRIORITY_LABEL, STATE_KEYS, stateLabelKey } from "@/lib/features";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import type { FeaturePriority, FeatureRow, FeatureState, FeatureStore } from "@/lib/types";
import type { useFeatureWrite } from "./useWrite";

export function FeatureDetail(p: { row: FeatureRow; write: ReturnType<typeof useFeatureWrite>; onUpdated: (r: FeatureRow) => void }) {
  const { t } = useT();
  const { toast } = useApp();
  const { row } = p;
  const [store, setStore] = useState<FeatureStore>("runtime");
  const [state, setState] = useState<FeatureState>("enabled");
  const [variant, setVariant] = useState(0);
  const [priority, setPriority] = useState<FeaturePriority>(DEFAULT_PRIORITY);

  // When the row or the store changes, reload from that store's existing configuration.
  useEffect(() => {
    const cur = store === "runtime" ? row.runtime : row.boot;
    setState(cur?.state ?? "enabled");
    setVariant(cur?.variant ?? 0);
    setPriority(cur && EDITABLE_PRIORITIES.includes(cur.priority) ? cur.priority : DEFAULT_PRIORITY);
  }, [row, store]);

  const reload = () => api.featuresGet(row.id).then(p.onUpdated);
  const apply = () =>
    p.write.run(async () => {
      await api.featuresSet(row.id, store, state, clampVariant(variant), priority);
      toast("ok", t(store === "boot" ? "features.toast.applied_boot" : "features.toast.applied"));
      await reload();
    });
  const reset = () =>
    p.write.run(async () => {
      await api.featuresReset(row.id, store, priority);
      toast("ok", t("features.toast.reset"));
      await reload();
    });

  return (
    <Group title={row.name ?? t("features.row.unnamed")} en={`ID ${row.id}`}>
      <Row label={t("features.field.store")}>
        <Segmented<FeatureStore>
          options={[
            { value: "runtime", label: t("features.store.runtime") },
            { value: "boot", label: t("features.store.boot") },
          ]}
          value={store}
          onChange={setStore}
        />
      </Row>
      {store === "boot" && <p className="lb-hint">{t("features.store.boot_note")}</p>}
      <Row label={t("features.field.state")}>
        <Segmented<FeatureState> options={STATE_KEYS.map((s) => ({ value: s, label: t(stateLabelKey(s)) }))} value={state} onChange={setState} />
      </Row>
      <Row label={t("features.field.variant")}>
        <input
          className="wp-input ft-num"
          type="number"
          min={0}
          max={63}
          value={variant}
          aria-label={t("features.field.variant")}
          onChange={(e) => setVariant(clampVariant(e.target.valueAsNumber))}
        />
      </Row>
      <Row label={t("features.field.priority")}>
        <SelectBox<FeaturePriority>
          options={EDITABLE_PRIORITIES.map((x) => ({ value: x, label: PRIORITY_LABEL[x] ?? x }))}
          value={priority}
          onChange={setPriority}
          label={t("features.field.priority")}
        />
      </Row>
      <Row>
        <JoinButton label={t("features.detail.apply")} size="small" disabled={p.write.busy} onClick={() => void apply()} />
        <JoinButton label={t("features.card.reset")} variant="ghost" size="small" disabled={p.write.busy} onClick={() => void reset()} />
      </Row>
    </Group>
  );
}
