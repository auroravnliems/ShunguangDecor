"use client";
// Preview cursor: the frame-strip image (sheet) cropped to the frame being shown, advancing per `delaysMs`.
// Reduced-motion mode (html[data-motion="reduced"]) shows only the first frame.
import { useEffect, useState } from "react";

function reduced(): boolean {
  return typeof document !== "undefined" && document.documentElement.dataset.motion === "reduced";
}

/** Index of the frame being shown; switches frames following each delay exactly. */
export function useFrameIndex(delaysMs: number[]): number {
  const [idx, setIdx] = useState(0);
  const key = delaysMs.join(",");
  useEffect(() => {
    setIdx(0);
    if (delaysMs.length < 2) return;
    let i = 0;
    let timer: ReturnType<typeof setTimeout>;
    const step = () => {
      if (reduced()) {
        timer = setTimeout(step, 500);
        return;
      }
      i = (i + 1) % delaysMs.length;
      setIdx(i);
      timer = setTimeout(step, Math.max(16, delaysMs[i]));
    };
    timer = setTimeout(step, Math.max(16, delaysMs[0]));
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return reduced() ? 0 : idx;
}

export function CursorSprite(p: {
  /** URL of the frame-strip image (`sheet.png`). */
  url: string;
  frames: number;
  delaysMs: number[];
  /** Original size of one frame. */
  width: number;
  height: number;
  /** Longest side when displayed (px). */
  size: number;
  /** Scale up preserving individual pixels (Sharp). */
  pixelated?: boolean;
  className?: string;
}) {
  const idx = useFrameIndex(p.delaysMs);
  const long = Math.max(p.width, p.height, 1);
  const w = (p.width / long) * p.size;
  const h = (p.height / long) * p.size;
  const frames = Math.max(1, p.frames);
  return (
    <span
      className={`cur-sprite${p.pixelated ? " px" : ""}${p.className ? ` ${p.className}` : ""}`}
      style={{
        width: w,
        height: h,
        backgroundImage: `url("${p.url}")`,
        backgroundSize: `${frames * 100}% 100%`,
        backgroundPosition: `${frames > 1 ? (Math.min(idx, frames - 1) / (frames - 1)) * 100 : 0}% 0`,
      }}
      aria-hidden
    />
  );
}
