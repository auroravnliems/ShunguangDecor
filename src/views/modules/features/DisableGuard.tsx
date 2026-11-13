"use client";
// Turning off the Hidden Features switch: asks whether to undo the changes the app made (Yes / No / Cancel) before turning it off.
// If nothing has been changed (empty log), there is nothing to ask, so it turns off right away.
import { useEffect, useState } from "react";
import * as api from "@/lib/api";
import { undoTargetCount } from "@/lib/features";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import { isDeclined, useRefreshModules } from "./useWrite";

export interface DisableGuardProps {
  /** Allow turning the part off. */
  onProceed: () => void;
  /** Keep the part turned on. */
  onCancel: () => void;
}

export function DisableGuard({ onProceed, onCancel }: DisableGuardProps) {
  const { t, tt } = useT();
  const { toast } = useApp();
  const refresh = useRefreshModules();
  const [count, setCount] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    api
      .featuresUndoList()
      .then((l) => {
        if (!alive) return;
        if (l.length === 0) onProceed();
        else setCount(undoTargetCount(l));
      })
      .catch((e) => {
        if (!alive) return;
        toast("err", api.errorText(e, tt));
        onCancel();
      });
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const undoThenDisable = async () => {
    setBusy(true);
    try {
      await api.featuresUndoAll();
      void refresh();
      onProceed();
    } catch (e) {
      // If the undo does not complete, keep the part turned on so the user can still see it and retry.
      toast(isDeclined(e) ? "ok" : "err", isDeclined(e) ? t("features.strip.denied") : api.errorText(e, tt));
      onCancel();
    }
  };

  if (count === null) return null;
  return (
    <div className="overlay">
      <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="ft-dg-title">
        <h3 id="ft-dg-title">{t("features.disable.title")}</h3>
        <div className="body">{t("features.disable.body", { n: count })}</div>
        <div className="actions">
          <button className="jb ghost small" onClick={onCancel} disabled={busy}>
            <b>{t("dialog.cancel")}</b>
          </button>
          <button className="jb ghost small" onClick={onProceed} disabled={busy}>
            <b>{t("features.disable.skip")}</b>
          </button>
          <button className="jb small" onClick={() => void undoThenDisable()} disabled={busy} autoFocus>
            <b>{t("features.disable.undo")}</b>
            <span className="chev">
              <span>›››</span>
            </span>
          </button>
        </div>
      </div>
    </div>
  );
}
