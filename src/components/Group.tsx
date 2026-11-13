"use client";
// Control group (v9 .grp), secondary button (v9 .ib), loading skeleton (v9 .skel).
import type { ReactNode } from "react";

export function Group({ title, en, children, className }: { title: string; en: string; children: ReactNode; className?: string }) {
  return (
    <div className={`grp${className ? ` ${className}` : ""}`}>
      <h5>
        {title}
        <small>{en}</small>
      </h5>
      {children}
    </div>
  );
}

export function Row({ label, children }: { label?: string; children: ReactNode }) {
  return (
    <div className="row">
      {label !== undefined && <label>{label}</label>}
      {children}
    </div>
  );
}

export function IconButton(p: { label: string; onClick?: () => void; disabled?: boolean; title?: string }) {
  return (
    <button className="ib" onClick={p.onClick} disabled={p.disabled} title={p.title ?? p.label}>
      {p.label}
    </button>
  );
}

export function Skeleton({ rows }: { rows: number }) {
  return (
    <div className="skel">
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="a" style={{ ["--d" as string]: `${0.2 + i * 0.08}s` }} />
      ))}
    </div>
  );
}
