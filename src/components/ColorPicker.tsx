"use client";
// Custom-drawn color picker (not <input type=color>, because WebKitGTK/WKWebView currently differ):
// a square for saturation/brightness, a hue slider, a hex input box. Esc or clicking outside closes it.
import { useEffect, useRef, useState } from "react";
import { hexToHsv, hsvToHex, normalizeHex, type Hsv } from "@/lib/color";

export function ColorPicker({ value, onChange, onClose }: { value: string; onChange: (hex: string) => void; onClose: () => void }) {
  const [hsv, setHsv] = useState<Hsv>(() => hexToHsv(value) ?? { h: 218, s: 0.75, v: 0.91 });
  const [text, setText] = useState(value);
  const box = useRef<HTMLDivElement>(null);
  const sv = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  const commit = (next: Hsv) => {
    setHsv(next);
    const hex = hsvToHex(next.h, next.s, next.v);
    setText(hex);
    onChange(hex);
  };

  const pickSv = (clientX: number, clientY: number) => {
    const r = sv.current?.getBoundingClientRect();
    if (!r || r.width === 0) return;
    const s = Math.min(1, Math.max(0, (clientX - r.left) / r.width));
    const v = Math.min(1, Math.max(0, 1 - (clientY - r.top) / r.height));
    commit({ ...hsv, s, v });
  };

  const hex = hsvToHex(hsv.h, hsv.s, hsv.v);
  const bad = normalizeHex(text) === null;
  return (
    <div className="picker" ref={box}>
      <div
        className="sv"
        ref={sv}
        style={{ backgroundColor: hsvToHex(hsv.h, 1, 1) }}
        onPointerDown={(e) => {
          (e.target as HTMLElement).setPointerCapture(e.pointerId);
          pickSv(e.clientX, e.clientY);
        }}
        onPointerMove={(e) => e.buttons === 1 && pickSv(e.clientX, e.clientY)}
      >
        <i style={{ left: `${hsv.s * 100}%`, top: `${(1 - hsv.v) * 100}%`, background: hex }} />
      </div>
      <input className="hue" type="range" min={0} max={359} value={Math.round(hsv.h)} onChange={(e) => commit({ ...hsv, h: Number(e.target.value) })} />
      <div className="hexrow">
        <span style={{ background: hex }} />
        <input
          className={bad ? "bad" : undefined}
          value={text}
          spellCheck={false}
          onChange={(e) => {
            setText(e.target.value);
            const n = normalizeHex(e.target.value);
            const next = n ? hexToHsv(n) : null;
            if (n && next) {
              setHsv(next);
              onChange(n);
            }
          }}
        />
      </div>
    </div>
  );
}
