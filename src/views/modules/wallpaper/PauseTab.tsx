"use client";
// Auto-pause tab: the auto-pause rules, the app list, the check interval.
import { useState } from "react";
import { Group, Row } from "@/components/Group";
import { RangeSlider } from "@/components/RangeSlider";
import { Toggle } from "@/components/Toggle";
import { useT } from "@/lib/i18n";
import type { WallpaperConfig } from "@/lib/wallpaper";
import type { WallpaperCtx } from "./ctx";

type BoolKey = {
  [K in keyof WallpaperConfig]: WallpaperConfig[K] extends boolean ? K : never;
}[keyof WallpaperConfig];

const RULES: { key: BoolKey; label: string }[] = [
  { key: "pause_when_covered", label: "wallpaper.pause.covered" },
  { key: "pause_when_app_focused", label: "wallpaper.pause.focused" },
  { key: "pause_per_monitor", label: "wallpaper.pause.per_monitor" },
  { key: "pause_on_battery", label: "wallpaper.pause.battery" },
  { key: "pause_on_battery_saver", label: "wallpaper.pause.saver" },
  { key: "pause_on_remote_desktop", label: "wallpaper.pause.remote" },
  { key: "trim_memory", label: "wallpaper.pause.trim" },
];

/** Normalizes the exe name the user typed (strips whitespace and extra path); empty → null. */
export function cleanExeName(s: string): string | null {
  const name = s.trim().split(/[\\/]/).pop()?.trim() ?? "";
  return name ? name : null;
}

export function PauseTab({ ctx }: { ctx: WallpaperCtx }) {
  const { t } = useT();
  const { cfg, update } = ctx;
  const [app, setApp] = useState("");
  const add = () => {
    const name = cleanExeName(app);
    if (!name) return;
    if (!cfg.pause_apps.some((a) => a.toLowerCase() === name.toLowerCase())) update({ pause_apps: [...cfg.pause_apps, name] });
    setApp("");
  };
  return (
    <div className="wp-pause">
      <Group title={t("wallpaper.tab.pause")} en="AUTO-PAUSE">
        {RULES.map((r) => (
          <Row key={r.key} label="">
            <Toggle on={cfg[r.key]} onChange={(v) => update({ [r.key]: v } as Partial<WallpaperConfig>)} label={t(r.label)} />
            <span className="wp-rule">{t(r.label)}</span>
          </Row>
        ))}
        <Row label={t("wallpaper.pause.interval")}>
          <RangeSlider min={250} max={10000} step={250} value={cfg.poll_interval_ms} onChange={(poll_interval_ms) => update({ poll_interval_ms })} format={(n) => t("wallpaper.pause.interval_ms", { n })} label={t("wallpaper.pause.interval")} />
        </Row>
      </Group>
      <Group title={t("wallpaper.pause.apps")} en="APPS">
        <div className="wp-chips">
          {cfg.pause_apps.map((a) => (
            <span key={a} className="chip">
              {a}
              <button aria-label={t("wallpaper.pause.apps_remove", { name: a })} onClick={() => update({ pause_apps: cfg.pause_apps.filter((x) => x !== a) })}>
                ×
              </button>
            </span>
          ))}
        </div>
        <div className="wp-url">
          <input value={app} placeholder={t("wallpaper.pause.apps_placeholder")} spellCheck={false} onChange={(e) => setApp(e.target.value)} onKeyDown={(e) => e.key === "Enter" && add()} />
          <button className="ib" disabled={!cleanExeName(app)} onClick={add}>
            {t("wallpaper.pause.apps_add")}
          </button>
        </div>
      </Group>
    </div>
  );
}
