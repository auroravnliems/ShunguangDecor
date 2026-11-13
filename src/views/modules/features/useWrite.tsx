"use client";
// Shared write gate of the Hidden features page: every write goes through here. The first write in a session shows a warning box
// ("I understand"); errors from Rust always show up as a notice; clicking "No" in the UAC prompt is normal, not an error.
import { useCallback, useState, type ReactNode } from "react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import * as api from "@/lib/api";
import { acknowledgeWarning, isWarningAcknowledged } from "@/lib/features";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";

/** Rust error code when admin rights are missing (the user clicked "No" in the UAC prompt). */
export const NO_ADMIN_KEY = "features.err.no_admin";
export const isDeclined = (e: unknown) => !!e && typeof e === "object" && (e as { key?: unknown }).key === NO_ADMIN_KEY;

/** Re-reads the module info (counts of changed items; the admin-rights state is in `info.status.details`). */
export function useRefreshModules() {
  const { applyModule } = useApp();
  return useCallback(
    () =>
      api
        .listModules()
        .then((ms) => ms.forEach(applyModule))
        .catch((e) => api.logFrontend("warn", `refresh modules failed: ${String(e)}`)),
    [applyModule],
  );
}

export interface WriteOptions {
  /** Restore-only actions (Undo all) skip the warning prompt. */
  skipWarning?: boolean;
}

export function useFeatureWrite(onChanged: () => void) {
  const { t, tt } = useT();
  const { toast } = useApp();
  const refresh = useRefreshModules();
  const [warning, setWarning] = useState<((ok: boolean) => void) | null>(null);
  const [busy, setBusy] = useState(false);
  /** The most recent write was denied permission (so the top strip can show a gentle reminder). */
  const [denied, setDenied] = useState(false);

  const askWarning = () =>
    isWarningAcknowledged() ? Promise.resolve(true) : new Promise<boolean>((resolve) => setWarning(() => resolve));

  /** Runs one write; returns the result, or `undefined` if cancelled / denied / failed (the user has already been told). */
  const run = async <T,>(job: () => Promise<T>, opt: WriteOptions = {}): Promise<T | undefined> => {
    if (!opt.skipWarning && !(await askWarning())) return undefined;
    setBusy(true);
    try {
      const r = await job();
      setDenied(false);
      return r;
    } catch (e) {
      if (isDeclined(e)) {
        setDenied(true);
        toast("ok", t("features.strip.denied"));
      } else toast("err", api.errorText(e, tt));
      return undefined;
    } finally {
      setBusy(false);
      onChanged();
      void refresh();
    }
  };

  const dialog: ReactNode = warning && (
    <ConfirmDialog
      title={t("features.warn.title")}
      message={t("features.warn.body")}
      confirmLabel={t("features.warn.ok")}
      onConfirm={() => {
        acknowledgeWarning();
        warning(true);
        setWarning(null);
      }}
      onCancel={() => {
        warning(false);
        setWarning(null);
      }}
    />
  );

  return { run, busy, denied, setDenied, dialog };
}
