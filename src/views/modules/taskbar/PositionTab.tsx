"use client";
import { Group, Row } from "@/components/Group";
import { RangeSlider } from "@/components/RangeSlider";
import { Segmented } from "@/components/Segmented";
import { useT } from "@/lib/i18n";
import { type BarStyle, type Edge, HEIGHT, MARGIN, type NativeMode, type WinButtonPos } from "@/lib/taskbar";
import type { TabProps } from "./tabProps";

const NATIVE: NativeMode[] = ["keep", "hide", "reveal"];

export function PositionTab({ cfg, set }: TabProps) {
  const { t } = useT();
  const px = (n: number) => t("taskbar.val.px", { n });
  return (
    <>
      <Group title={t("taskbar.group.position")} en="POSITION & SIZE">
        <Row label={t("taskbar.pos.edge")}>
          <Segmented<Edge>
            options={[
              { value: "top", label: t("taskbar.edge.top") },
              { value: "bottom", label: t("taskbar.edge.bottom") },
            ]}
            value={cfg.edge}
            onChange={(edge) => set({ edge })}
          />
        </Row>
        <Row label={t("taskbar.pos.height")}>
          <RangeSlider min={HEIGHT.min} max={HEIGHT.max} value={cfg.height} onChange={(height) => set({ height })} format={px} label={t("taskbar.pos.height")} />
        </Row>
        <Row label={t("taskbar.pos.style")}>
          <Segmented<BarStyle>
            options={[
              { value: "docked", label: t("taskbar.style.docked") },
              { value: "floating", label: t("taskbar.style.floating") },
            ]}
            value={cfg.style}
            onChange={(style) => set({ style })}
          />
        </Row>
        {cfg.style === "floating" && (
          <Row label={t("taskbar.pos.margin")}>
            <RangeSlider min={MARGIN.min} max={MARGIN.max} value={cfg.margin} onChange={(margin) => set({ margin })} format={px} label={t("taskbar.pos.margin")} />
          </Row>
        )}
        <Row label={t("taskbar.pos.winbtn")}>
          <Segmented<WinButtonPos>
            options={[
              { value: "left", label: t("taskbar.winbtn.left") },
              { value: "center", label: t("taskbar.winbtn.center") },
            ]}
            value={cfg.windows_button}
            onChange={(windows_button) => set({ windows_button })}
          />
        </Row>
      </Group>
      <Group title={t("taskbar.group.native")} en="NATIVE TASKBAR">
        <Row>
          <Segmented<NativeMode> options={NATIVE.map((m) => ({ value: m, label: t(`taskbar.native.${m}`) }))} value={cfg.native} onChange={(native) => set({ native })} />
        </Row>
        <p className="lb-hint">{t(`taskbar.native.hint.${cfg.native}`)}</p>
      </Group>
    </>
  );
}
