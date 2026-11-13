"use client";
// A square select box with a dropdown list; arrow keys/Enter/Esc work.
import { useEffect, useRef, useState } from "react";

export function SelectBox<T extends string>(p: {
  options: { value: T; label: string }[];
  value: T | null;
  placeholder?: string;
  onChange: (v: T) => void;
  disabled?: boolean;
  label?: string;
}) {
  const [open, setOpen] = useState(false);
  const [hi, setHi] = useState(0);
  const box = useRef<HTMLDivElement>(null);
  const current = p.options.find((o) => o.value === p.value);

  useEffect(() => {
    if (!open) return;
    setHi(Math.max(0, p.options.findIndex((o) => o.value === p.value)));
    const onDown = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open, p.options, p.value]);

  const pick = (v: T) => {
    setOpen(false);
    if (v !== p.value) p.onChange(v);
  };
  const onKey = (e: React.KeyboardEvent) => {
    if (!open || p.options.length === 0) return;
    if (e.key === "Escape") setOpen(false);
    else if (e.key === "ArrowDown") setHi((h) => (h + 1) % p.options.length);
    else if (e.key === "ArrowUp") setHi((h) => (h + p.options.length - 1) % p.options.length);
    else if (e.key === "Enter") pick(p.options[hi].value);
    else return;
    e.preventDefault();
  };

  return (
    <div className="selw" ref={box} onKeyDown={onKey}>
      <button
        className="sel"
        disabled={p.disabled}
        aria-label={p.label}
        aria-haspopup="listbox"
        aria-expanded={open}
        title={current?.label ?? p.placeholder}
        onClick={() => setOpen((o) => !o)}
      >
        <span>{current?.label ?? p.placeholder ?? ""}</span>
      </button>
      {open && p.options.length > 0 && (
        <div className="menu" role="listbox">
          {p.options.map((o, i) => (
            <button
              key={o.value}
              role="option"
              aria-selected={o.value === p.value}
              className={[o.value === p.value ? "on" : "", i === hi ? "hi" : ""].join(" ").trim() || undefined}
              onMouseEnter={() => setHi(i)}
              onClick={() => pick(o.value)}
            >
              <span className="ellipsis">{o.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
