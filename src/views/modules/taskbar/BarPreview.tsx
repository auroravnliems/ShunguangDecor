"use client";
// A mock bar drawn over the real desktop, built from the configuration itself (edge, height, floating/flush, color, opacity, blur, font,
// widgets in 3 zones). Sizes are computed from the frame width (cqw) against a reference 1920 px screen, so the bar scales proportionally.
import type { CSSProperties } from "react";
import { normalizeHex } from "@/lib/color";
import { clockOptions, effectiveZones, kindOf, optionsOf, type TaskbarConfig, type WidgetItem, ZONE_NAMES } from "@/lib/taskbar";

function rgba(hex: string, alpha: number): string {
  const h = (normalizeHex(hex) ?? "#000000").slice(1);
  const n = (i: number) => parseInt(h.slice(i, i + 2), 16);
  return `rgba(${n(0)}, ${n(2)}, ${n(4)}, ${Math.round(alpha * 100) / 100})`;
}

const SVG = { viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", strokeWidth: 2.4, strokeLinecap: "round", strokeLinejoin: "round" } as const;

function clockText(item: WidgetItem): string {
  const o = clockOptions(optionsOf(item));
  const time = o.hour12 ? "6:42" : "18:42";
  return `${time}${o.seconds ? ":07" : ""}${o.hour12 ? " PM" : ""}`;
}

function Glyph({ item }: { item: WidgetItem }) {
  switch (kindOf(item)) {
    case "windows_button":
      return (
        <span className="tbp-win">
          <i />
          <i />
          <i />
          <i />
        </span>
      );
    case "search":
      return (
        <svg className="tbp-ico" {...SVG}>
          <circle cx="11" cy="11" r="6" />
          <path d="m20 20-4-4" />
        </svg>
      );
    case "apps":
      return (
        <span className="tbp-apps">
          <i style={{ background: "#f2b13b" }} />
          <i style={{ background: "#4b8cf5" }} />
          <i style={{ background: "#e4572e" }} />
        </span>
      );
    case "tray":
      return <span className="tbp-txt">˄</span>;
    case "clock":
      return <span className="tbp-txt">{clockText(item)}</span>;
    case "volume":
      return (
        <svg className="tbp-ico" {...SVG}>
          <path d="M4 9v6h4l5 4V5L8 9z" />
          <path d="M16 9a4 4 0 0 1 0 6" />
        </svg>
      );
    case "network":
      return (
        <svg className="tbp-ico" {...SVG}>
          <path d="M2 9a15 15 0 0 1 20 0M5 13a10 10 0 0 1 14 0M8.5 16.5a5 5 0 0 1 7 0" />
          <circle cx="12" cy="20" r="1" />
        </svg>
      );
    case "battery":
      return (
        <svg className="tbp-ico" {...SVG}>
          <rect x="2" y="7" width="18" height="10" />
          <path d="M22 11v2" />
          <rect x="4" y="9" width="10" height="6" fill="currentColor" />
        </svg>
      );
    case "notifications":
      return (
        <svg className="tbp-ico" {...SVG}>
          <path d="M6 16V11a6 6 0 0 1 12 0v5l2 2H4z" />
          <path d="M10 21h4" />
        </svg>
      );
    case "cpu":
      return <span className="tbp-txt">CPU 12%</span>;
    case "memory":
      return <span className="tbp-txt">RAM 41%</span>;
    case "active_window":
      return <span className="tbp-line" />;
    case "power_menu":
      return (
        <svg className="tbp-ico" {...SVG}>
          <path d="M12 3v9" />
          <path d="M6.3 6.6a8 8 0 1 0 11.4 0" />
        </svg>
      );
    default:
      return <span className="tbp-line" />;
  }
}

export function BarPreview({ cfg }: { cfg: TaskbarConfig }) {
  const zones = effectiveZones(cfg);
  const floating = cfg.style === "floating";
  const style = {
    "--h": cfg.height,
    "--m": floating ? cfg.margin : 0,
    "--fs": cfg.font_size,
    "--fc": cfg.text_color,
    "--ac": cfg.accent,
    "--bg": rgba(cfg.background, cfg.opacity / 100),
    "--ff": `"${cfg.font_family}", "Segoe UI", sans-serif`,
  } as CSSProperties;
  return (
    <div className="tbp">
      <div className={`tbp-bar ${cfg.edge}${floating ? " float" : ""} blur-${cfg.blur}`} style={style}>
        {ZONE_NAMES.map((z) => (
          <div key={z} className={`tbp-zone ${z}`}>
            {zones[z].map((w) => (
              <span key={kindOf(w)} className="tbp-w">
                <Glyph item={w} />
              </span>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}
