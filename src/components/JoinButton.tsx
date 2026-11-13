"use client";
// The v9 "join"-style primary button: bold text + a small English line + a looping ››› arrow.
export function JoinButton(p: {
  label: string;
  sub?: string;
  variant?: "solid" | "ghost";
  size?: "normal" | "small";
  danger?: boolean;
  disabled?: boolean;
  title?: string;
  onClick?: () => void;
}) {
  const cls = ["jb", p.variant === "ghost" ? "ghost" : "", p.size === "small" ? "small" : "", p.danger ? "danger" : ""]
    .filter(Boolean)
    .join(" ");
  return (
    <button className={cls} onClick={p.onClick} disabled={p.disabled} title={p.title ?? p.label}>
      <b>{p.label}</b>
      {p.sub && <i>{p.sub}</i>}
      {!(p.variant === "ghost" && p.size === "small") && (
        <span className="chev">
          <span>›››</span>
        </span>
      )}
    </button>
  );
}
