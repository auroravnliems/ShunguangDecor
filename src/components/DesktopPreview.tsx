"use client";
// Preview frame showing the real desktop, shared by every page.
// The core captures ~2 images/second only while this frame is mounted; images swap using two stacked <img> layers
// (the new layer only shows once loaded, so there is no flicker). Before the first frame, the current wallpaper image is shown.
// Video/GIF wallpaper: the bottom layer plays that very file (at the right fps), the top layer is a PNG holding only the icons ("icons" capture mode).
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";
import { aspectFor, fallbackVisible, liveLayerKind, monitorLabelParts, newViewerId, objectFitFor, previewMode } from "@/lib/preview";
import { useApp } from "@/lib/store";
import type { LiveSource, MonitorView, WallpaperView } from "@/lib/types";

/** Reduced motion: only the app's own switch decides (html[data-motion="reduced"]; the store has already merged the "Follow Windows" option). */
function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(false);
  useEffect(() => {
    const read = () => setReduced(document.documentElement.dataset.motion === "reduced");
    read();
    const mo = new MutationObserver(read);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-motion"] });
    return () => mo.disconnect();
  }, []);
  return reduced;
}

export function DesktopPreview(p: {
  /** Monitor number (1, 2…). Omit for the primary monitor. */
  monitor?: number;
  onMonitorChange?: (n: number) => void;
  children?: ReactNode;
  className?: string;
}) {
  const { t } = useT();
  const { info } = useApp();
  const base = info?.protocolBase ?? "";
  const [monitors, setMonitors] = useState<MonitorView[]>([]);
  const [own, setOwn] = useState<number | null>(null);
  const [viewer] = useState(newViewerId);
  const [fallback, setFallback] = useState<WallpaperView | null>(null);
  // Two stacked image layers: `front` is the one currently shown.
  const [layers, setLayers] = useState<[string, string]>(["", ""]);
  const [front, setFront] = useState<0 | 1>(0);
  const [live, setLive] = useState(false);
  const frontRef = useRef<0 | 1>(0);
  frontRef.current = front;
  const reduced = useReducedMotion();
  // The video/GIF source running on this monitor (null = static/other wallpaper type → the old capture path).
  // The query result is tied to the monitor number that was asked about; until the query finishes for the selected monitor, `captureStart` is not called.
  const [resolved, setResolved] = useState<{ monitor: number; source: LiveSource | null } | null>(null);
  // Url of the source that has played its first frame ("" = not yet).
  const [readyUrl, setReadyUrl] = useState("");
  // Url of the source that reported a playback error in the WebView: treated as having no direct source (fall back to capturing the whole desktop).
  const [failedUrl, setFailedUrl] = useState("");

  useEffect(() => {
    api.listMonitors().then(setMonitors).catch(() => setMonitors([]));
    api.getCurrentWallpaper().then(setFallback).catch(() => setFallback(null));
  }, []);

  const selected = p.monitor ?? own ?? monitors.find((m) => m.primary)?.number ?? monitors[0]?.number ?? 1;
  const isResolved = resolved?.monitor === selected;
  const source = isResolved ? resolved.source : null;
  const layerKind = liveLayerKind(source, reduced, failedUrl);
  const liveSource = layerKind ? source : null;
  const mode = previewMode(isResolved, layerKind);
  const current = monitors.find((m) => m.number === selected) ?? null;
  const aspect = useMemo(() => aspectFor(current), [current]);

  // Ask the core for the video/GIF wallpaper of the monitor being viewed; ask again when the wallpaper or configuration changes.
  // Count the queries: only accept the result of the latest one (overlapping queries may return out of order).
  useEffect(() => {
    let cancelled = false;
    let latest = 0;
    let lastUrl: string | null = null;
    const apply = (s: LiveSource | null) => {
      const url = s?.url ?? "";
      // The source changed: drop the old source's "ready" state and reload the current wallpaper image as the placeholder.
      setReadyUrl((r) => (r === url ? r : ""));
      if (lastUrl !== null && lastUrl !== url) api.getCurrentWallpaper().then((w) => !cancelled && setFallback(w)).catch(() => {});
      lastUrl = url;
      // For the same file, keep the same object so the <video> is not rebuilt (and does not flicker).
      setResolved((old) => {
        const o = old?.monitor === selected ? old.source : null;
        const same = o && s && o.url === s.url && o.kind === s.kind && o.objectFit === s.objectFit && o.paused === s.paused;
        return same ? old : { monitor: selected, source: s };
      });
    };
    const query = () => {
      const my = ++latest;
      api
        .captureLiveSource(selected)
        .then((s) => !cancelled && my === latest && apply(s))
        .catch(() => !cancelled && my === latest && apply(null));
    };
    query();
    const offs = [api.onWallpaperState(() => query()), api.onModuleConfigChanged((id) => id === "wallpaper" && query()), api.onCaptureLiveChanged(() => query())];
    return () => {
      cancelled = true;
      offs.forEach((o) => o.then((f) => f()).catch(() => {}));
    };
  }, [selected]);

  // Tell the core which monitor this viewer is watching, in which mode, and receive the returned frames. Changing monitor/mode only calls
  // `capture_start` again (not `capture_stop`): back-to-back start/stop commands may reach the core out of order, and a wrong stop
  // would freeze the preview. With a video/GIF wallpaper → capture only the icon layer ("icons"), otherwise capture the whole desktop ("full").
  // Runs only once the source of the selected monitor is known (mode != null). `listen()` is asynchronous and the core immediately emits
  // the frame it already has when `capture_start` arrives, so `capture_start` is called only AFTER the listener has finished registering.
  useEffect(() => {
    setLive(false);
    setLayers(["", ""]);
    setFront(0);
    if (!mode) return;
    let cancelled = false;
    const off = api.onCaptureFrame((f) => {
      if (cancelled || f.monitor !== selected) return;
      // Drop frames of the old mode (still in transit).
      if (f.icons !== (mode === "icons")) return;
      const back = frontRef.current === 0 ? 1 : 0;
      setLayers((l) => {
        const n: [string, string] = [...l];
        n[back] = f.icons ? `${base}/capture/${selected}-icons.png?seq=${f.seq}` : `${base}/capture/${selected}.jpg?seq=${f.seq}`;
        return n;
      });
    });
    const start = () => {
      if (!cancelled) api.captureStart(viewer, selected, mode).catch(() => {});
    };
    off.then(start, start);
    return () => {
      cancelled = true;
      off.then((f) => f()).catch(() => {});
    };
  }, [viewer, selected, mode, base]);

  // Frame unmounted: fully stop capturing for this viewer.
  useEffect(() => () => void api.captureStop(viewer).catch(() => {}), [viewer]);

  // The back layer finished loading → bring it to the front.
  const shown = (i: 0 | 1) => {
    if (i !== (front === 0 ? 1 : 0) || !layers[i]) return;
    setFront(i);
    setLive(true);
  };

  // Live playback layer: shown only after the first frame is ready; before that (or on error) keep the current wallpaper image.
  const showMedia = liveSource !== null && readyUrl === liveSource.url;
  const fit = liveSource ? liveSource.objectFit ?? objectFitFor(liveSource.scaling) : "cover";
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const mediaUrl = liveSource?.url ?? "";
  // Source changed / monitor changed / frame unmounted: stop the video and remove `src` to release the decoder; back to not-ready.
  useEffect(() => {
    const el = videoRef.current;
    return () => {
      if (!el) return;
      el.pause();
      el.removeAttribute("src");
      el.load();
    };
  }, [mediaUrl, layerKind]);
  // Paused (by the user / an auto-pause rule) or reduced motion → stay on the first frame; otherwise play.
  const paused = !!liveSource?.paused || reduced;
  useEffect(() => {
    const el = videoRef.current;
    if (!el || layerKind !== "video") return;
    if (paused) el.pause();
    else el.play().catch(() => {});
  }, [paused, mediaUrl, layerKind, showMedia]);

  const pick = (n: number) => (p.onMonitorChange ? p.onMonitorChange(n) : setOwn(n));
  return (
    <div className={`dpv${p.className ? ` ${p.className}` : ""}`} style={{ aspectRatio: aspect }}>
      {fallbackVisible(showMedia, mode, live) && fallback && <img className="dpv-img on" src={fallback.url} alt="" draggable={false} />}
      {liveSource && layerKind === "video" && (
        <video
          key={liveSource.url}
          ref={videoRef}
          className={`dpv-img dpv-media${showMedia ? " on" : ""}`}
          style={{ objectFit: fit }}
          src={liveSource.url}
          muted
          loop
          playsInline
          preload={paused ? "metadata" : "auto"}
          disablePictureInPicture
          onLoadedData={() => setReadyUrl(liveSource.url)}
          onLoadStart={() => setReadyUrl((r) => (r === liveSource.url ? "" : r))}
          onError={() => {
            setReadyUrl("");
            setFailedUrl(liveSource.url);
          }}
        />
      )}
      {liveSource && layerKind === "gif" && (
        <img
          key={liveSource.url}
          className={`dpv-img dpv-media${showMedia ? " on" : ""}`}
          style={{ objectFit: fit }}
          src={liveSource.url}
          alt=""
          draggable={false}
          onLoad={() => setReadyUrl(liveSource.url)}
          onError={() => {
            setReadyUrl("");
            setFailedUrl(liveSource.url);
          }}
        />
      )}
      {([0, 1] as const).map(
        (i) =>
          layers[i] && (
            <img key={i} className={`dpv-img${live && front === i ? " on" : ""}`} src={layers[i]} alt="" draggable={false} onLoad={() => shown(i)} />
          ),
      )}
      {fallbackVisible(showMedia, mode, live) && !fallback && <div className="dpv-empty">{t("capture.waiting")}</div>}
      <div className="dpv-over">{p.children}</div>
      {monitors.length >= 2 && (
        <div className="dpv-pick" role="radiogroup" aria-label={t("capture.pick")}>
          {monitors.map((m) => {
            const l = monitorLabelParts(m);
            return (
              <button
                key={m.number}
                role="radio"
                aria-checked={m.number === selected}
                className={m.number === selected ? "on" : undefined}
                title={`${t("capture.monitor", { n: l.n })} — ${l.size}`}
                onClick={() => pick(m.number)}
              >
                {l.n}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
