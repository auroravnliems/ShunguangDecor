"use client";
// Fixed decoration around the page: polka dots, vertical blue bar, vertical text, large outlined text, page number, scroll marker.
import { useEffect, useState } from "react";
import { BIGWORD, RAIL, dotsShift, pageNo } from "@/lib/nav";
import type { PageId } from "@/lib/types";

export function Decor({ page }: { page: PageId }) {
  const [word, setWord] = useState(BIGWORD[page]);
  const [swap, setSwap] = useState(false);

  // The large text fades out, then the text is swapped (as in v9: 450 ms).
  useEffect(() => {
    if (BIGWORD[page] === word) return;
    setSwap(true);
    const t = setTimeout(() => {
      setWord(BIGWORD[page]);
      setSwap(false);
    }, 450);
    return () => clearTimeout(t);
  }, [page, word]);

  const [top, height] = RAIL[page];
  return (
    <>
      <div
        className="dots"
        style={{ left: 0, top: 210, width: 170, height: 210, transform: `translateY(${dotsShift(page)}px)` }}
      />
      <div className="cross" style={{ left: 196, top: 196 }} />
      <div className="rail" style={{ ["--rt" as string]: `${top}px`, ["--rh" as string]: height }} />
      <div className="vtext">SHUNGUANG · DECOR</div>
      <div className={`bigword${swap ? " swap" : ""}`}>{word}</div>
      <div className="pageno">{pageNo(page)}</div>
      <div className="scrolline" />
    </>
  );
}
