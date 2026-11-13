"use client";
// Custom-drawn minimize and close buttons, square style. Close = hide to the tray (decided by the Rust core).
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";

export function WindowButtons() {
  const { t } = useT();
  return (
    <div className="wbtns">
      <button className="wbtn" title={t("header.minimize")} onClick={() => api.minimizeWindow()}>
        <svg viewBox="0 0 10 10">
          <path d="M1 5h8" stroke="currentColor" strokeWidth="1.3" />
        </svg>
      </button>
      <button className="wbtn close" title={t("header.close")} onClick={() => api.closeWindow()}>
        <svg viewBox="0 0 10 10">
          <path d="M1.5 1.5l7 7M8.5 1.5l-7 7" stroke="currentColor" strokeWidth="1.3" />
        </svg>
      </button>
    </div>
  );
}
