"use client";
// Confirmation dialog with beveled corners. Esc = cancel, Enter = confirm. For dangerous actions, keyboard focus starts on the Cancel button.
import { useEffect, useRef, type ReactNode } from "react";
import { useT } from "@/lib/i18n";

export interface ConfirmProps {
  title: string;
  message: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  /** Hide the cancel button (the dialog is informational only). */
  noCancel?: boolean;
  danger?: boolean;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog(p: ConfirmProps) {
  const { t } = useT();
  const okRef = useRef<HTMLButtonElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    (p.danger && !p.noCancel ? cancelRef : okRef).current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !p.noCancel) {
        e.preventDefault();
        p.onCancel();
      } else if (e.key === "Enter" && document.activeElement !== cancelRef.current && !p.busy) {
        e.preventDefault();
        p.onConfirm();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [p.onConfirm, p.onCancel, p.busy]);

  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && !p.noCancel && p.onCancel()}>
      <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="dlg-title">
        <h3 id="dlg-title">{p.title}</h3>
        <div className="body">{p.message}</div>
        <div className="actions">
          {!p.noCancel && (
            <button ref={cancelRef} className="jb ghost small" onClick={p.onCancel} disabled={p.busy}>
              <b>{p.cancelLabel ?? t("dialog.cancel")}</b>
            </button>
          )}
          <button ref={okRef} className={`jb small${p.danger ? " danger" : ""}`} onClick={p.onConfirm} disabled={p.busy}>
            <b>{p.confirmLabel ?? t("dialog.ok")}</b>
            <span className="chev">
              <span>›››</span>
            </span>
          </button>
        </div>
      </div>
    </div>
  );
}
