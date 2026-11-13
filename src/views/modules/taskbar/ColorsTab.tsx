"use client";
import { Group, Row } from "@/components/Group";
import { RangeSlider } from "@/components/RangeSlider";
import { Segmented } from "@/components/Segmented";
import { Swatches } from "@/components/Swatches";
import { useT } from "@/lib/i18n";
import type { Blur } from "@/lib/taskbar";
import type { TabProps } from "./tabProps";

const BG_COLORS = ["#1c1c1c", "#000000", "#ffffff", "#141826", "#3b7be9", "#e889c1"];
const ACCENT_COLORS = ["#4cc2ff", "#3b7be9", "#e889c1", "#ffc24b", "#29c483"];
const BLURS: Blur[] = ["none", "blur", "acrylic"];

export function ColorsTab({ cfg, set }: TabProps) {
  const { t } = useT();
  return (
    <>
      <Group title={t("taskbar.group.bg")} en="BACKGROUND">
        <Row label={t("taskbar.colors.color")}>
          <Swatches colors={BG_COLORS} value={cfg.background} onChange={(background) => set({ background })} />
        </Row>
        <Row label={t("taskbar.colors.opacity")}>
          <RangeSlider min={0} max={100} value={cfg.opacity} onChange={(opacity) => set({ opacity })} format={(n) => t("taskbar.val.pct", { n })} label={t("taskbar.colors.opacity")} />
        </Row>
        <Row label={t("taskbar.colors.blur")}>
          <Segmented<Blur> options={BLURS.map((b) => ({ value: b, label: t(`taskbar.blur.${b}`) }))} value={cfg.blur} onChange={(blur) => set({ blur })} />
        </Row>
      </Group>
      <Group title={t("taskbar.group.accent")} en="ACCENT">
        <Row label={t("taskbar.colors.accent")}>
          <Swatches colors={ACCENT_COLORS} value={cfg.accent} onChange={(accent) => set({ accent })} />
        </Row>
      </Group>
    </>
  );
}
