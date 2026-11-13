"use client";
import { Group, Row } from "@/components/Group";
import { SelectBox } from "@/components/SelectBox";
import { Toggle } from "@/components/Toggle";
import { useT } from "@/lib/i18n";
import { autoHideBlocked as isAutoHideBlocked, CORNERS, type HotCorner } from "@/lib/taskbar";
import type { TabProps } from "./tabProps";

const HOT: HotCorner[] = ["none", ...CORNERS];

export function BehaviorTab({ cfg, set }: TabProps) {
  const { t } = useT();
  // The native taskbar in "hidden, touch the edge to reveal" mode sits on the bottom edge by default: if our bar is also on the bottom edge, both fight for it,
  // and the engine drops "Auto-hide" in that case, so the toggle must be dimmed and show OFF (without overwriting the saved value: once the
  // blocking mode is removed, the toggle goes back to the value the user chose). The page doesn't know the native taskbar's real edge.
  const autoHideBlocked = isAutoHideBlocked(cfg);
  const toggle = (key: "hide_on_fullscreen" | "all_monitors" | "auto_hide", label: string, disabled = false) => (
    <Row>
      <Toggle on={cfg[key] && !disabled} onChange={(v) => set({ [key]: v })} label={label} disabled={disabled} />
      <span className="lb-rule">{label}</span>
    </Row>
  );
  return (
    <Group title={t("taskbar.group.behavior")} en="BEHAVIOR">
      {toggle("hide_on_fullscreen", t("taskbar.behavior.fullscreen"))}
      <Row>
        <span className="lb-rule">{t("taskbar.behavior.hot_corner")}</span>
        <SelectBox<HotCorner>
          options={HOT.map((c) => ({ value: c, label: t(`taskbar.corner.${c}`) }))}
          value={cfg.hot_corner}
          onChange={(hot_corner) => set({ hot_corner })}
          label={t("taskbar.behavior.hot_corner")}
        />
      </Row>
      {toggle("all_monitors", t("taskbar.behavior.all_monitors"))}
      {toggle("auto_hide", t("taskbar.behavior.auto_hide"), autoHideBlocked)}
      {autoHideBlocked && <p className="lb-hint">{t("taskbar.behavior.auto_hide_conflict")}</p>}
    </Group>
  );
}
