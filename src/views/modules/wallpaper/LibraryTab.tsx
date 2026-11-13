"use client";
// Library tab: thumbnail grid, filter by type, paste a URL, import Lively; clicking a card = set it for the selected screen.
import { useEffect, useState } from "react";
import { Segmented } from "@/components/Segmented";
import { useT } from "@/lib/i18n";
import { parseSourceMissing, type WallpaperItem, usedBy } from "@/lib/wallpaper";
import type { WallpaperCtx } from "./ctx";

type Filter = "all" | "video" | "gif" | "picture" | "web" | "app";

const FILTER_KINDS: Record<Exclude<Filter, "all">, string[]> = {
  video: ["video", "videostream"],
  gif: ["gif"],
  picture: ["picture"],
  web: ["web", "webaudio", "url"],
  app: ["app", "unity", "godot", "bizhawk", "unityaudio"],
};

export function matchesFilter(kind: string, f: Filter): boolean {
  return f === "all" || FILTER_KINDS[f].includes(kind);
}

function kindLabelKey(kind: string): string {
  if (kind === "videostream") return "wallpaper.kind.videostream";
  if (kind === "webaudio") return "wallpaper.kind.web";
  if (["video", "gif", "picture", "web", "url"].includes(kind)) return `wallpaper.kind.${kind}`;
  return "wallpaper.kind.app";
}

export function LibraryTab(p: {
  ctx: WallpaperCtx;
  busy: boolean;
  onSet: (item: WallpaperItem) => void;
  onAddFile: () => void;
  onAddUrl: (url: string) => void;
  onImportZip: () => void;
  onImportFolder: () => void;
  onRename: (item: WallpaperItem) => void;
  onDelete: (item: WallpaperItem) => void;
  onCustomize: (item: WallpaperItem) => void;
}) {
  const { t } = useT();
  const { ctx } = p;
  const [filter, setFilter] = useState<Filter>("all");
  const [url, setUrl] = useState("");
  const [menuFor, setMenuFor] = useState<string | null>(null);

  useEffect(() => {
    if (!menuFor) return;
    const close = () => setMenuFor(null);
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [menuFor]);

  const shown = ctx.items.filter((i) => matchesFilter(i.kind, filter));
  const submitUrl = () => {
    const v = url.trim();
    if (!v) return;
    p.onAddUrl(v);
    setUrl("");
  };
  const used = (id: string): string | null => {
    const keys = usedBy(ctx.cfg, id);
    if (keys.length === 0 || !ctx.info.enabled) return null;
    return keys.includes("all") ? t("wallpaper.lib.in_use_span") : t("wallpaper.lib.in_use", { n: keys.filter((k) => k !== "all").join(", ") });
  };

  return (
    <div className="wp-lib">
      <div className="wp-bar">
        <Segmented<Filter>
          options={[
            { value: "all", label: t("wallpaper.lib.filter_all") },
            { value: "video", label: t("wallpaper.kind.video") },
            { value: "gif", label: t("wallpaper.kind.gif") },
            { value: "picture", label: t("wallpaper.kind.picture") },
            { value: "web", label: t("wallpaper.kind.web") },
            { value: "app", label: t("wallpaper.kind.app") },
          ]}
          value={filter}
          onChange={setFilter}
        />
        <div className="wp-url">
          <input
            value={url}
            placeholder={t("wallpaper.lib.url_placeholder")}
            onChange={(e) => setUrl(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && submitUrl()}
            spellCheck={false}
          />
          <button className="ib" disabled={p.busy || !url.trim()} onClick={submitUrl}>
            {t("wallpaper.lib.url_add")}
          </button>
        </div>
        <button className="ib" disabled={p.busy} onClick={p.onImportZip}>
          {t("wallpaper.lib.import_zip")}
        </button>
        <button className="ib" disabled={p.busy} onClick={p.onImportFolder}>
          {t("wallpaper.lib.import_folder")}
        </button>
      </div>
      <div className="wp-grid">
        {ctx.items.length === 0 && <div className="wp-empty">{t("wallpaper.lib.empty")}</div>}
        {shown.map((it, i) => {
          const missing = parseSourceMissing(ctx.state.errors[it.id]) !== null || !it.sourceExists;
          const useText = used(it.id);
          const err = ctx.state.errors[it.id];
          return (
            <div
              key={it.id}
              className={`wp-card${useText ? " on" : ""}${missing ? " bad" : ""}`}
              role="button"
              tabIndex={0}
              style={{ animationDelay: `${Math.min(i, 12) * 0.03}s` }}
              title={missing ? t("wallpaper.err.source_missing", { path: it.source }) : err || it.source}
              onClick={() => !missing && p.onSet(it)}
              onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && !missing && p.onSet(it)}
            >
              <div className="th">
                {it.thumbUrl ? <img src={it.thumbUrl} alt="" draggable={false} loading="lazy" /> : <span className="ph">{t(kindLabelKey(it.kind))}</span>}
                <span className="kd">{t(kindLabelKey(it.kind))}</span>
                {missing && <span className="bd">{t("wallpaper.lib.broken")}</span>}
              </div>
              <div className="nm ellipsis">{it.title}</div>
              {useText && <div className="use">{useText}</div>}
              <button
                className="more"
                aria-label={t("wallpaper.lib.menu")}
                aria-haspopup="menu"
                onMouseDown={(e) => e.stopPropagation()}
                onClick={(e) => {
                  e.stopPropagation();
                  setMenuFor((m) => (m === it.id ? null : it.id));
                }}
              >
                …
              </button>
              {menuFor === it.id && (
                <div className="wp-menu" role="menu" onMouseDown={(e) => e.stopPropagation()}>
                  {it.hasProperties && (
                    <button role="menuitem" onClick={() => (setMenuFor(null), p.onCustomize(it))}>
                      {t("wallpaper.lib.customize")}
                    </button>
                  )}
                  <button role="menuitem" onClick={() => (setMenuFor(null), p.onRename(it))}>
                    {t("wallpaper.lib.rename")}
                  </button>
                  <button role="menuitem" className="danger" onClick={() => (setMenuFor(null), p.onDelete(it))}>
                    {t("wallpaper.lib.delete")}
                  </button>
                </div>
              )}
            </div>
          );
        })}
        <div className="wp-card add" role="button" tabIndex={0} onClick={() => !p.busy && p.onAddFile()} onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && !p.busy && p.onAddFile()}>
          <b>{t("wallpaper.lib.add_file")}</b>
        </div>
      </div>
    </div>
  );
}
