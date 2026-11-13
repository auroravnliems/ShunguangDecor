"use client";
// Pick one of a few options (v9 .seg).
export function Segmented<T extends string | number>(p: {
  options: { value: T; label: string }[];
  value: T;
  onChange: (v: T) => void;
  disabled?: boolean;
}) {
  return (
    <div className="seg" role="radiogroup">
      {p.options.map((o) => (
        <button
          key={String(o.value)}
          role="radio"
          aria-checked={o.value === p.value}
          disabled={p.disabled}
          className={o.value === p.value ? "on" : undefined}
          onClick={() => p.onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
