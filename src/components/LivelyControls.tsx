"use client";
// The Lively customization panel (LivelyProperties.json) drawn with the app's own components:
// slider, checkbox, dropdown, color, text box, folder, button, label. A changed value → onChange after 120 ms (dragging a slider doesn't fire commands continuously).
import { useEffect, useRef, useState } from "react";
import { useT } from "@/lib/i18n";
import { hexToHsv, normalizeHex } from "@/lib/color";
import { controlKind, type LivelyControl } from "@/lib/wallpaper";
import { ColorPicker } from "./ColorPicker";
import { RangeSlider } from "./RangeSlider";
import { SelectBox } from "./SelectBox";
import { Toggle } from "./Toggle";

/** Delay before sending the new value (ms). */
export const CONTROL_DELAY = 120;

function labelOf(name: string, c: LivelyControl): string {
  return (c.text ?? "").trim() || name;
}

function Control(p: {
  name: string;
  c: LivelyControl;
  value: unknown;
  onValue: (v: unknown) => void;
  onPress: () => void;
}) {
  const { t } = useT();
  const [picking, setPicking] = useState(false);
  const { c, value } = p;
  switch (controlKind(c)) {
    case "slider": {
      const min = Number(c.min ?? 0);
      const max = Number(c.max ?? 100);
      const step = Number(c.step ?? 1) || 1;
      const n = typeof value === "number" ? value : Number(value ?? min);
      return (
        <RangeSlider
          min={min}
          max={max}
          step={step}
          value={Number.isFinite(n) ? n : min}
          onChange={p.onValue}
          format={(v) => String(Math.round(v * 100) / 100)}
          label={labelOf(p.name, c)}
        />
      );
    }
    case "checkbox":
      return <Toggle on={Boolean(value)} onChange={p.onValue} label={labelOf(p.name, c)} />;
    case "dropdown": {
      const items = (c.items ?? []).map(String);
      const idx = typeof value === "number" ? value : Number(value ?? 0);
      return (
        <SelectBox
          options={items.map((label, i) => ({ value: String(i), label }))}
          value={String(Number.isFinite(idx) ? idx : 0)}
          onChange={(v) => p.onValue(Number(v))}
          label={labelOf(p.name, c)}
        />
      );
    }
    case "folderDropdown": {
      const items = (c.items ?? []).map(String);
      return (
        <SelectBox
          options={[{ value: "", label: t("wallpaper.props.none_option") }, ...items.map((f) => ({ value: f, label: f }))]}
          value={typeof value === "string" ? value : ""}
          onChange={(v) => p.onValue(v === "" ? null : v)}
          label={labelOf(p.name, c)}
        />
      );
    }
    case "color": {
      const hex = typeof value === "string" && hexToHsv(value) ? (normalizeHex(value) ?? "#000000") : "#000000";
      return (
        <div className="lc-color">
          <button className="lc-sw" style={{ background: hex }} aria-label={labelOf(p.name, c)} onClick={() => setPicking((x) => !x)} />
          <span className="hint">{hex}</span>
          {picking && <ColorPicker value={hex} onChange={p.onValue} onClose={() => setPicking(false)} />}
        </div>
      );
    }
    case "textbox":
      return <input className="lc-text" value={typeof value === "string" ? value : String(value ?? "")} onChange={(e) => p.onValue(e.target.value)} spellCheck={false} />;
    case "button":
      return (
        <button className="jb ghost small" onClick={p.onPress}>
          <b>{t("wallpaper.props.press")}</b>
        </button>
      );
    default:
      return null;
  }
}

export function LivelyControls(p: {
  controls: Record<string, LivelyControl>;
  /** Initial value (changes when the wallpaper differs or on "Reset to default"). */
  values: Record<string, unknown>;
  onChange: (name: string, value: unknown) => void;
  onPress: (name: string) => void;
}) {
  const [local, setLocal] = useState<Record<string, unknown>>(p.values);
  const timers = useRef<Record<string, ReturnType<typeof setTimeout>>>({});
  const onChange = useRef(p.onChange);
  onChange.current = p.onChange;

  useEffect(() => setLocal(p.values), [p.values]);
  useEffect(
    () => () => {
      for (const id of Object.values(timers.current)) clearTimeout(id);
    },
    [],
  );

  const set = (name: string, value: unknown) => {
    setLocal((l) => ({ ...l, [name]: value }));
    clearTimeout(timers.current[name]);
    timers.current[name] = setTimeout(() => onChange.current(name, value), CONTROL_DELAY);
  };

  return (
    <div className="lc">
      {Object.entries(p.controls).map(([name, c]) => {
        if (controlKind(c) === "label") {
          return (
            <div key={name} className="row lc-label">
              <span className="hint">{labelOf(name, c)}</span>
            </div>
          );
        }
        return (
          <div key={name} className="row" title={c.help || undefined}>
            <label>{labelOf(name, c)}</label>
            <Control name={name} c={c} value={local[name]} onValue={(v) => set(name, v)} onPress={() => p.onPress(name)} />
          </div>
        );
      })}
    </div>
  );
}
