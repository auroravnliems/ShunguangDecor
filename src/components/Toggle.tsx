"use client";
// Beveled-corner switch. When disabled it is dimmed and does not change; Space/Enter work because it is a <button>.
export function Toggle(p: { on: boolean; onChange: (on: boolean) => void; disabled?: boolean; title?: string; label?: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={p.on}
      aria-label={p.label ?? p.title}
      title={p.title}
      disabled={p.disabled}
      className={`tg${p.on ? " on" : ""}`}
      onClick={(e) => {
        e.stopPropagation();
        if (!p.disabled) p.onChange(!p.on);
      }}
    />
  );
}
