"use client";
// Home page: time-of-day greeting, the 「」 quote, status + real RAM, two big buttons,
// the machine's real wallpaper, 6 module cards. The thumbnail strip and "View fullscreen" come with the Wallpaper module.
import { useEffect, useState } from "react";
import { JoinButton } from "@/components/JoinButton";
import { ModuleCard } from "@/components/ModuleCard";
import * as api from "@/lib/api";
import { importIntent } from "@/lib/cursor";
import { formatMb } from "@/lib/format";
import { greetingKey, quoteKey } from "@/lib/greeting";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import { useModuleToggle } from "./modules/useModuleToggle";
import type { ModuleInfo, WallpaperView } from "@/lib/types";
import { setWallpaperOn } from "@/lib/wallpaperApply";
import type { WallpaperItem } from "@/lib/wallpaper";

const RAM_POLL_MS = 5000;

function useNow(): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 60_000);
    return () => clearInterval(id);
  }, []);
  return now;
}

function Wallpaper() {
  const { t, tt } = useT();
  const { modules, applyModule, toast } = useApp();
  const [strip, setStrip] = useState<WallpaperItem[]>([]);
  const [wp, setWp] = useState<WallpaperView | null | undefined>(undefined);
  const [loaded, setLoaded] = useState(false);
  const [broken, setBroken] = useState(false);
  useEffect(() => {
    api
      .getCurrentWallpaper()
      .then(setWp)
      .catch(() => setWp(null));
  }, []);
  useEffect(() => {
    const load = () =>
      api
        .wallpaperLibrary()
        .then((l) => setStrip(l.slice(0, 6)))
        .catch(() => setStrip([]));
    load();
    const off = api.onWallpaperLibraryChanged(load);
    return () => {
      off.then((f) => f());
    };
  }, []);
  const wpOn = modules.find((m) => m.id === "wallpaper")?.enabled ?? false;
  const pickStrip = (id: string) =>
    api
      .listMonitors()
      .then((ms) => setWallpaperOn(ms.find((m) => m.primary)?.number ?? ms[0]?.number ?? 1, id, wpOn))
      .then((info) => {
        if (info) applyModule(info);
        // The large image follows the wallpaper just chosen.
        api.getCurrentWallpaper().then(setWp).catch(() => {});
      })
      .catch((e) => toast("err", api.errorText(e, tt)));
  const name = wp ? wp.name || t("home.wallpaper.windows") : wp === null ? t("home.wallpaper.none") : "";
  return (
    <div className="hero a" style={{ ["--d" as string]: ".3s" }}>
      <div className="hatch">
        <div className="tl">
          MAKE YOUR DESKTOP
          <br />A WORLD OF YOUR OWN
        </div>
        <div className="pk" />
      </div>
      <div className="info">
        <div className="nm ellipsis" title={name}>
          {name}
        </div>
        {wp && <div className="sub">{t("home.wallpaper.sub")}</div>}
      </div>
      <div className="shot">
        {wp && !broken && (
          <img className={`scn${loaded ? " on" : ""}`} src={wp.url} alt="" draggable={false} onLoad={() => setLoaded(true)} onError={() => setBroken(true)} />
        )}
        {(wp === null || broken) && <div className="empty">{t("home.wallpaper.none_hint")}</div>}
      </div>
      {strip.length > 0 && (
        <div className="strip" role="group" aria-label={t("home.btn.wallpaper")}>
          {strip.map((it) => (
            <button key={it.id} title={it.title} aria-label={it.title} onClick={() => void pickStrip(it.id)}>
              {it.thumbUrl && <img src={it.thumbUrl} alt="" draggable={false} />}
            </button>
          ))}
        </div>
      )}
      <div className="blue-sq" />
    </div>
  );
}

export function HomeView() {
  const { t } = useT();
  const { modules, navigate } = useApp();
  const now = useNow();
  const [ram, setRam] = useState<number | null>(null);

  useEffect(() => {
    let alive = true;
    const tick = () =>
      api
        .getRuntimeStats()
        .then((s) => alive && setRam(s.ramBytes))
        .catch(() => {});
    tick();
    const id = setInterval(tick, RAM_POLL_MS);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, []);

  const on = modules.filter((m) => m.enabled).length;
  const faulted = modules.filter((m) => m.fault).length;
  const health = faulted ? t("home.health.faulted", { count: faulted }) : t("home.health.ok");
  const { request, guardNode } = useModuleToggle();
  const toggle = (m: ModuleInfo, want: boolean) => request(m.id, want);

  return (
    <>
      {guardNode}
      <div className="home-l">
        <div className="a" style={{ ["--d" as string]: ".15s" }}>
          <span className="sl">{t("nav.home")}</span>
          <span className="en">HOME · OVERVIEW</span>
        </div>
        <div className="h1 a" style={{ ["--d" as string]: ".25s", marginTop: 22 }}>
          {t(greetingKey(now.getHours()))}
        </div>
        <div className="quote a" style={{ ["--d" as string]: ".35s" }}>
          <b>「</b>
          {t(quoteKey(now))}
          <b>」</b>
        </div>
        <div className="stat a" style={{ ["--d" as string]: ".45s" }}>
          <span className={`sq${faulted ? " bad" : ""}`} />
          <span>
            <b>
              {on} / {modules.length}
            </b>{" "}
            {t("home.status_tail", { health, ram: formatMb(ram) })}
          </span>
        </div>
        <div className="home-btns a" style={{ ["--d" as string]: ".55s" }}>
          <JoinButton label={t("home.btn.wallpaper")} sub="CHANGE WALLPAPER" onClick={() => navigate("wallpaper")} />
          <JoinButton
            label={t("home.btn.cursor")}
            sub="IMPORT CURSORS"
            variant="ghost"
            onClick={() => {
              importIntent.pending = true;
              navigate("cursor");
            }}
          />
        </div>
      </div>
      <Wallpaper />
      <div className="mods">
        {modules.map((m, i) => (
          <ModuleCard key={m.id} info={m} index={i} onOpen={() => navigate(m.id)} onToggle={(want) => toggle(m, want)} />
        ))}
      </div>
    </>
  );
}
