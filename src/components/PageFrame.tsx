"use client";
// A "page" with the same effect as v9: when newly mounted, `.show` is added after 2 frames so the transition runs;
// the old page gets `.leave` (slides up, fades out) before being removed.
import { useEffect, useState, type ReactNode } from "react";

export type FrameState = "enter" | "wait" | "leave";

export function PageFrame({ state, children }: { state: FrameState; children: ReactNode }) {
  const [shown, setShown] = useState(false);
  useEffect(() => {
    if (state !== "enter") return;
    let r2 = 0;
    const r1 = requestAnimationFrame(() => {
      r2 = requestAnimationFrame(() => setShown(true));
    });
    return () => {
      cancelAnimationFrame(r1);
      cancelAnimationFrame(r2);
    };
  }, [state]);
  const cls = state === "leave" ? "page leave" : shown ? "page show" : "page";
  return <section className={cls}>{children}</section>;
}
