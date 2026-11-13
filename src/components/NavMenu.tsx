"use client";
// Horizontal menu with 7 items. Two-layer rolling text on hover, scaleX underline.
// Long translations make the menu overflow → it switches to the compact style automatically.
import { useLayoutEffect, useRef, useState } from "react";
import { useT } from "@/lib/i18n";
import { NAV_PAGES } from "@/lib/nav";
import type { PageId } from "@/lib/types";

export function NavMenu({ page, onGo }: { page: PageId; onGo: (p: PageId) => void }) {
  const { t, lang } = useT();
  const ref = useRef<HTMLElement>(null);
  const [compact, setCompact] = useState(false);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      el.classList.remove("compact");
      const over = el.scrollWidth > el.clientWidth + 1;
      el.classList.toggle("compact", over);
      setCompact(over);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el.parentElement ?? el);
    // The embedded font finishes loading after the first paint and is wider than the fallback font: re-measure, or the menu gets clipped without going compact.
    let alive = true;
    document.fonts?.ready.then(() => alive && measure());
    document.fonts?.addEventListener("loadingdone", measure);
    return () => {
      alive = false;
      ro.disconnect();
      document.fonts?.removeEventListener("loadingdone", measure);
    };
  }, [lang]);

  return (
    <nav ref={ref} className={compact ? "compact" : undefined}>
      {NAV_PAGES.map((p) => {
        const label = t(`nav.${p}`);
        return (
          <button key={p} className={p === page ? "on" : undefined} onClick={() => onGo(p)} title={label}>
            <span className="roll">
              <span className="ellipsis">{label}</span>
              <span className="ellipsis">{label}</span>
            </span>
          </button>
        );
      })}
    </nav>
  );
}
