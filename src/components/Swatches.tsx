"use client";
// Preset color swatches + a last swatch that opens the color picker (v9 .sw).
import { useState } from "react";
import { useT } from "@/lib/i18n";
import { ColorPicker } from "./ColorPicker";

export function Swatches({ colors, value, onChange }: { colors: string[]; value: string; onChange: (hex: string) => void }) {
  const { t } = useT();
  const [open, setOpen] = useState(false);
  const custom = !colors.includes(value.toLowerCase());
  return (
    <div className="sw">
      {colors.map((c) => (
        <button key={c} title={c} aria-label={c} className={c === value.toLowerCase() ? "on" : undefined} style={{ background: c }} onClick={() => onChange(c)} />
      ))}
      <button
        className={`pick${custom ? " on" : ""}`}
        title={t("control.color_picker")}
        aria-label={t("control.color_picker")}
        onClick={() => setOpen((o) => !o)}
      />
      {open && <ColorPicker value={value} onChange={onChange} onClose={() => setOpen(false)} />}
    </div>
  );
}
