"use client";
// Slider filled in blue according to its value (v9 .rng) + a number box on the right.
export function RangeSlider(p: {
  min: number;
  max: number;
  step?: number;
  value: number;
  onChange: (v: number) => void;
  format: (v: number) => string;
  disabled?: boolean;
  label?: string;
}) {
  const pct = p.max > p.min ? ((p.value - p.min) / (p.max - p.min)) * 100 : 0;
  return (
    <div className="rngw">
      <input
        className="rng"
        type="range"
        aria-label={p.label}
        min={p.min}
        max={p.max}
        step={p.step ?? 1}
        value={p.value}
        disabled={p.disabled}
        style={{ ["--p" as string]: `${pct}%` }}
        onChange={(e) => p.onChange(Number(e.target.value))}
      />
      <span className="val">{p.format(p.value)}</span>
    </div>
  );
}
