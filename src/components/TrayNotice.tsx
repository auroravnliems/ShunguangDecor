"use client";
// The first time the window is closed: tell the user the app keeps running in the tray, then close.
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import { ConfirmDialog } from "./ConfirmDialog";

export function TrayNotice() {
  const { t } = useT();
  const { trayNotice, setTrayNotice } = useApp();
  if (!trayNotice) return null;
  const ok = async () => {
    setTrayNotice(false);
    await api.ackTrayNotice().catch(() => {});
    await api.closeWindow();
  };
  return (
    <ConfirmDialog
      title={t("dialog.tray_notice.title")}
      message={t("dialog.tray_notice.body")}
      confirmLabel={t("dialog.tray_notice.ok")}
      noCancel
      onConfirm={ok}
      onCancel={ok}
    />
  );
}
