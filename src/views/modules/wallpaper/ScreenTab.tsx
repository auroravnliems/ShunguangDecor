"use client";
// Screen tab: the real desktop + monitor picker + layout + fill mode.
import { DesktopPreview } from "@/components/DesktopPreview";
import { Group, IconButton, Row } from "@/components/Group";
import { Segmented } from "@/components/Segmented";
import { useT } from "@/lib/i18n";
import { type Arrangement, layoutForArrangement, layoutKey, type Scaling, withoutWallpaper } from "@/lib/wallpaper";
import type { WallpaperCtx } from "./ctx";

export function ScreenTab({ ctx }: { ctx: WallpaperCtx }) {
  const { t } = useT();
  const { cfg, update, items, monitor, monitors } = ctx;
  const key = layoutKey(cfg.arrangement, monitor);
  const current = items.find((i) => i.id === cfg.layout[key]);
  return (
    <div className="wp-screen">
      <div className="wp-pv">
        <DesktopPreview monitor={monitor} onMonitorChange={ctx.setMonitor} />
        <small>{t("wallpaper.screen.preview")}</small>
      </div>
      <div className="wp-side">
        <Group title={t("wallpaper.screen.arrangement")} en="ARRANGEMENT">
          <Segmented<Arrangement>
            options={[
              { value: "per_monitor", label: t("wallpaper.screen.per_monitor") },
              { value: "span", label: t("wallpaper.screen.span") },
              { value: "duplicate", label: t("wallpaper.screen.duplicate") },
            ]}
            value={cfg.arrangement}
            onChange={(arrangement) => update({ arrangement, layout: layoutForArrangement(cfg, arrangement) })}
          />
          {cfg.arrangement === "per_monitor" && monitors.length >= 2 && (
            <Row label={t("wallpaper.screen.target")}>
              <Segmented<number>
                options={monitors.map((m) => ({ value: m.number, label: String(m.number) }))}
                value={monitor}
                onChange={ctx.setMonitor}
              />
            </Row>
          )}
          <Row>
            <span className="hint ellipsis">{cfg.arrangement === "per_monitor" ? "" : t("wallpaper.screen.target_all")}</span>
            <span className="hint ellipsis" title={current?.title}>
              {current?.title ?? "—"}
            </span>
            {current && <IconButton label={t("wallpaper.lib.clear", { n: cfg.arrangement === "per_monitor" ? monitor : "∗" })} onClick={() => update({ layout: withoutWallpaper(cfg, monitor) })} />}
          </Row>
        </Group>
        <Group title={t("wallpaper.screen.scaling")} en="FIT MODE">
          <Segmented<Scaling>
            options={[
              { value: "fill", label: t("wallpaper.screen.fill") },
              { value: "fit", label: t("wallpaper.screen.fit") },
              { value: "stretch", label: t("wallpaper.screen.stretch") },
            ]}
            value={cfg.scaling}
            onChange={(scaling) => update({ scaling })}
          />
        </Group>
      </div>
    </div>
  );
}
