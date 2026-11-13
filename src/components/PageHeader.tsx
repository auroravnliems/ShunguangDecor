"use client";
// Header of a part's page (v9 .ph): title + English subtitle line, on/off switch, right-hand area (profile picker…).
import type { ReactNode } from "react";
import { useT } from "@/lib/i18n";
import { Toggle } from "./Toggle";

export function PageHeader(p: {
  title: string;
  en: string;
  toggle?: { on: boolean; disabled?: boolean; reason?: string; onChange: (on: boolean) => void };
  right?: ReactNode;
}) {
  const { t } = useT();
  return (
    <div className="ph">
      <div className="a" style={{ ["--d" as string]: ".1s" }}>
        <div className="t">{p.title}</div>
        <span className="en">{p.en}</span>
      </div>
      {p.toggle && (
        <div className="tgw a" style={{ ["--d" as string]: ".15s" }} title={p.toggle.reason}>
          <Toggle on={p.toggle.on} disabled={p.toggle.disabled} title={p.toggle.reason} label={p.title} onChange={p.toggle.onChange} />
          {p.toggle.on ? t("module.state.on") : t("module.state.off")}
        </div>
      )}
      {p.right && (
        <div className="prof a" style={{ ["--d" as string]: ".2s" }}>
          {p.right}
        </div>
      )}
    </div>
  );
}
