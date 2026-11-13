"use client";
import { useEffect, useMemo, useState } from "react";
import { Group, Row } from "@/components/Group";
import { RangeSlider } from "@/components/RangeSlider";
import { SelectBox } from "@/components/SelectBox";
import { Swatches } from "@/components/Swatches";
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";
import { FALLBACK_FONTS, FONT_SIZE } from "@/lib/taskbar";
import type { TabProps } from "./tabProps";

const TEXT_COLORS = ["#ffffff", "#22262f", "#4cc2ff", "#ffd1e8", "#ffc24b"];

export function TextTab({ cfg, set }: TabProps) {
  const { t } = useT();
  const [fonts, setFonts] = useState<string[] | null>(null);
  useEffect(() => {
    api.taskbarListFonts().then(setFonts).catch(() => setFonts([]));
  }, []);
  const options = useMemo(() => {
    const list = fonts && fonts.length > 0 ? fonts : FALLBACK_FONTS;
    const all = list.includes(cfg.font_family) ? list : [cfg.font_family, ...list];
    return all.map((f) => ({ value: f, label: f }));
  }, [fonts, cfg.font_family]);
  return (
    <Group title={t("taskbar.group.text")} en="TYPOGRAPHY">
      <Row label={t("taskbar.text.font")}>
        <SelectBox options={options} value={cfg.font_family} onChange={(font_family) => set({ font_family })} label={t("taskbar.text.font")} />
      </Row>
      <Row label={t("taskbar.text.size")}>
        <RangeSlider
          min={FONT_SIZE.min}
          max={FONT_SIZE.max}
          value={cfg.font_size}
          onChange={(font_size) => set({ font_size })}
          format={(n) => t("taskbar.val.px", { n })}
          label={t("taskbar.text.size")}
        />
      </Row>
      <Row label={t("taskbar.text.color")}>
        <Swatches colors={TEXT_COLORS} value={cfg.text_color} onChange={(text_color) => set({ text_color })} />
      </Row>
    </Group>
  );
}
