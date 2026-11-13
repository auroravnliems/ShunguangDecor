"use client";
// Desktop text page: on the left, the real desktop captured again after applying (not simulated),
// on the right, Automatic / Custom. The status strip shows whether it is applied via the DLL or the fallback method.
import { useCallback, useEffect, useState } from "react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { DesktopPreview } from "@/components/DesktopPreview";
import { Group, IconButton, Row } from "@/components/Group";
import { RangeSlider } from "@/components/RangeSlider";
import { Segmented } from "@/components/Segmented";
import { Swatches } from "@/components/Swatches";
import { Toggle } from "@/components/Toggle";
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";
import {
  type LabelsConfig,
  type LabelsCustom,
  type LabelsMode,
  MAX_OUTLINE,
  MAX_SHADOW_OFFSET,
  normalizeLabelsConfig,
  OFF_STATUS,
  statusKind,
} from "@/lib/labels";
import { useApp } from "@/lib/store";
import type { LabelsStatus } from "@/lib/types";
import { useModuleConfig } from "@/lib/useModuleConfig";
import type { ModuleBodyProps } from "./index";

const TEXT_COLORS = ["#ffffff", "#101010", "#ff69b4", "#ffd166", "#7ec8ff"];
const EFFECT_COLORS = ["#000000", "#ffffff", "#1b2a4a", "#5a2a4a"];

export function LabelsView({ info }: ModuleBodyProps) {
  const { t, tt } = useT();
  const { toast } = useApp();
  const { cfg, update } = useModuleConfig<LabelsConfig>("labels", normalizeLabelsConfig);
  const [status, setStatus] = useState<LabelsStatus>(OFF_STATUS);
  const [asking, setAsking] = useState(false);
  const [restarting, setRestarting] = useState(false);

  useEffect(() => {
    api.labelsStatus().then(setStatus).catch(() => {});
    const off = api.onLabelsStatus(setStatus);
    return () => {
      off.then((f) => f());
    };
  }, [info.enabled]);

  const setCustom = useCallback(
    (patch: Partial<LabelsCustom>) => {
      if (cfg) update({ custom: { ...cfg.custom, ...patch } });
    },
    [cfg, update],
  );

  const restart = async () => {
    setAsking(false);
    setRestarting(true);
    try {
      await api.explorerRestart();
      toast("ok", t("labels.restart.done"));
    } catch (e) {
      toast("err", api.errorText(e, tt));
    }
    setRestarting(false);
  };

  const kind = info.enabled ? statusKind(status) : "off";
  const px = (n: number) => t("labels.px", { n });
  const pct = (n: number) => t("labels.pct", { n });

  return (
    <div className="cur-body lb-body">
      <div className={`notice a lb-state ${kind}`} style={{ ["--d" as string]: ".2s" }}>
        {kind === "off" && <b>{t("labels.state.off")}</b>}
        {kind === "dll" && (
          <>
            <b>{t("labels.state.dll")}</b>
            {status.mode === "auto" && <span>{t("labels.state.dll_auto", { n: status.items })}</span>}
          </>
        )}
        {kind === "fallback" && (
          <>
            <b>{t("labels.state.fallback")}</b>
            {status.error && <span>{tt(status.error)}</span>}
          </>
        )}
        {kind === "restart" && (
          <>
            <b>{t("labels.state.restart")}</b>
            <span>{t("labels.state.restart_hint")}</span>
            <IconButton label={t("labels.restart.button")} disabled={restarting} onClick={() => setAsking(true)} />
          </>
        )}
      </div>

      {cfg && (
        <div className="lb-main a" style={{ ["--d" as string]: ".28s" }}>
          <div className="lb-pv">
            <DesktopPreview />
            <small>{t("labels.preview.caption")}</small>
          </div>
          <div className="lb-side">
            <Group title={t("labels.mode.title")} en="MODE">
              <Segmented<LabelsMode>
                options={[
                  { value: "auto", label: t("labels.mode.auto") },
                  { value: "custom", label: t("labels.mode.custom") },
                ]}
                value={cfg.mode}
                onChange={(mode) => update({ mode })}
              />
            </Group>

            {cfg.mode === "auto" ? (
              <Group title={t("labels.mode.auto")} en="AUTO">
                <Row>
                  <Toggle on={cfg.outline} onChange={(outline) => update({ outline })} label={t("labels.auto.outline")} />
                  <span className="lb-rule">{t("labels.auto.outline")}</span>
                </Row>
                <p className="lb-hint">{t("labels.auto.hint")}</p>
              </Group>
            ) : (
              <>
                <Group title={t("labels.custom.text")} en="TEXT">
                  <Swatches colors={TEXT_COLORS} value={cfg.custom.color} onChange={(color) => setCustom({ color })} />
                </Group>
                <Group title={t("labels.custom.outline")} en="OUTLINE">
                  <Row label={t("labels.custom.color")}>
                    <Swatches colors={EFFECT_COLORS} value={cfg.custom.outline_color} onChange={(outline_color) => setCustom({ outline_color })} />
                  </Row>
                  <Row label={t("labels.custom.width")}>
                    <RangeSlider min={0} max={MAX_OUTLINE} value={cfg.custom.outline_width} onChange={(outline_width) => setCustom({ outline_width })} format={px} label={t("labels.custom.width")} />
                  </Row>
                </Group>
                <Group title={t("labels.custom.shadow")} en="SHADOW">
                  <Row label={t("labels.custom.color")}>
                    <Swatches colors={EFFECT_COLORS} value={cfg.custom.shadow_color} onChange={(shadow_color) => setCustom({ shadow_color })} />
                  </Row>
                  <Row label={t("labels.custom.strength")}>
                    <RangeSlider min={0} max={100} value={cfg.custom.shadow_alpha} onChange={(shadow_alpha) => setCustom({ shadow_alpha })} format={pct} label={t("labels.custom.strength")} />
                  </Row>
                  <Row label={t("labels.custom.dx")}>
                    <RangeSlider min={-MAX_SHADOW_OFFSET} max={MAX_SHADOW_OFFSET} value={cfg.custom.shadow_dx} onChange={(shadow_dx) => setCustom({ shadow_dx })} format={px} label={t("labels.custom.dx")} />
                  </Row>
                  <Row label={t("labels.custom.dy")}>
                    <RangeSlider min={-MAX_SHADOW_OFFSET} max={MAX_SHADOW_OFFSET} value={cfg.custom.shadow_dy} onChange={(shadow_dy) => setCustom({ shadow_dy })} format={px} label={t("labels.custom.dy")} />
                  </Row>
                </Group>
                <Group title={t("labels.custom.backdrop")} en="BACKDROP">
                  <Row label={t("labels.custom.color")}>
                    <Swatches colors={EFFECT_COLORS} value={cfg.custom.backdrop_color} onChange={(backdrop_color) => setCustom({ backdrop_color })} />
                  </Row>
                  <Row label={t("labels.custom.strength")}>
                    <RangeSlider min={0} max={100} value={cfg.custom.backdrop_alpha} onChange={(backdrop_alpha) => setCustom({ backdrop_alpha })} format={pct} label={t("labels.custom.strength")} />
                  </Row>
                </Group>
                {kind === "fallback" && <p className="lb-hint">{t("labels.custom.fallback_note")}</p>}
              </>
            )}
          </div>
        </div>
      )}

      {asking && (
        <ConfirmDialog
          title={t("labels.restart.title")}
          message={t("labels.restart.body")}
          confirmLabel={t("labels.restart.confirm")}
          danger
          busy={restarting}
          onCancel={() => setAsking(false)}
          onConfirm={() => void restart()}
        />
      )}
    </div>
  );
}
