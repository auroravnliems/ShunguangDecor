"use client";
// The on/off switch for a module, shared by the module pages and the Overview page: a module that has a prompt before turning off
// (MODULE_DISABLE_GUARDS) asks first, the others are called directly. `guardNode` must be rendered wherever the hook is used.
import { useState, type ReactNode } from "react";
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";
import { needsDisableGuard } from "@/lib/moduleGuard";
import { useApp } from "@/lib/store";
import type { ModuleId } from "@/lib/types";
import { MODULE_DISABLE_GUARDS } from "./index";

export function useModuleToggle(): { request: (id: ModuleId, want: boolean) => void; guardNode: ReactNode } {
  const { t, tt } = useT();
  const { applyModule, toast } = useApp();
  const [asking, setAsking] = useState<ModuleId | null>(null);

  const apply = (id: ModuleId, want: boolean) =>
    api
      .setModuleEnabled(id, want)
      .then(applyModule)
      .catch((e) => toast("err", `${t(`module.${id}.name`)}: ${api.errorText(e, tt)}`));
  const request = (id: ModuleId, want: boolean) => {
    if (needsDisableGuard(MODULE_DISABLE_GUARDS, id, want)) setAsking(id);
    else void apply(id, want);
  };
  const Guard = asking ? MODULE_DISABLE_GUARDS[asking] : undefined;
  const guardNode =
    asking && Guard ? (
      <Guard
        onProceed={() => {
          setAsking(null);
          void apply(asking, false);
        }}
        onCancel={() => setAsking(null)}
      />
    ) : null;
  return { request, guardNode };
}
