"use client";
// Short notifications in the bottom-right corner, auto-hidden after 4 seconds.
import { useApp } from "@/lib/store";

export function Toasts() {
  const { toasts, dismissToast } = useApp();
  return (
    <div className="toasts" aria-live="polite">
      {toasts.map((x) => (
        <button key={x.id} className={`toast ${x.kind}`} onClick={() => dismissToast(x.id)}>
          <i />
          <span>{x.text}</span>
        </button>
      ))}
    </div>
  );
}
