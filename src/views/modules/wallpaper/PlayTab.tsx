"use client";
// Playback tab: volume, video player (+ mpv), hardware decoding, pause/resume.
import { useState } from "react";
import { Group, IconButton, Row } from "@/components/Group";
import { RangeSlider } from "@/components/RangeSlider";
import { Segmented } from "@/components/Segmented";
import { Toggle } from "@/components/Toggle";
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";
import type { VideoPlayer } from "@/lib/wallpaper";
import type { WallpaperCtx } from "./ctx";

export function PlayTab({ ctx }: { ctx: WallpaperCtx }) {
  const { t } = useT();
  const { cfg, update, info, state } = ctx;
  const [found, setFound] = useState<string | null | undefined>(undefined);
  const findMpv = () =>
    api
      .wallpaperFindMpv()
      .then(setFound)
      .catch(ctx.fail);
  const toggle = () =>
    api
      .wallpaperTogglePause()
      .catch(ctx.fail);
  return (
    <div className="wp-play">
      <Group title={t("wallpaper.play.volume")} en="VOLUME">
        <Row label={t("wallpaper.play.volume")}>
          <RangeSlider min={0} max={100} step={1} value={cfg.volume} onChange={(volume) => update({ volume })} format={(n) => `${n}%`} label={t("wallpaper.play.volume")} />
        </Row>
        <Row label={t("wallpaper.play.pause")}>
          <IconButton label={state.manualPause ? t("wallpaper.play.resume") : t("wallpaper.play.pause")} onClick={() => void toggle()} disabled={!info.enabled} />
          <span className="hint">{state.manualPause ? t("wallpaper.play.state_paused") : t("wallpaper.play.state_running")}</span>
        </Row>
      </Group>
      <Group title={t("wallpaper.play.player")} en="VIDEO PLAYER">
        <Row label={t("wallpaper.play.player")}>
          <Segmented<VideoPlayer>
            options={[
              { value: "auto", label: t("wallpaper.play.auto") },
              { value: "media_foundation", label: t("wallpaper.play.mf") },
              { value: "mpv", label: t("wallpaper.play.mpv") },
            ]}
            value={cfg.video_player}
            onChange={(video_player) => update({ video_player })}
          />
        </Row>
        <Row label={t("wallpaper.play.mpv_path")}>
          <input className="wp-input" value={cfg.mpv_path} placeholder={t("wallpaper.play.mpv_placeholder")} spellCheck={false} onChange={(e) => update({ mpv_path: e.target.value })} />
          <IconButton label={t("wallpaper.play.find_mpv")} onClick={() => void findMpv()} />
        </Row>
        {found !== undefined && (
          <Row>
            <span className="hint ellipsis">{found ? t("wallpaper.play.mpv_found", { path: found }) : t("wallpaper.play.mpv_missing")}</span>
          </Row>
        )}
        <Row label={t("wallpaper.play.hw")}>
          <Toggle on={cfg.hardware_decoding} onChange={(hardware_decoding) => update({ hardware_decoding })} label={t("wallpaper.play.hw")} />
        </Row>
      </Group>
    </div>
  );
}
