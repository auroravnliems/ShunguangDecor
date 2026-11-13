"use client";
// Page of one part: a shared frame (title, toggle, profile bar) + that part's own page body
// (views/modules). A part without a real page yet shows a "coming soon" notice.
import { Skeleton } from "@/components/Group";
import { PageHeader } from "@/components/PageHeader";
import { ProfileBar } from "@/components/ProfileBar";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import type { ModuleId } from "@/lib/types";
import { MODULE_BODIES } from "./modules";
import { useModuleToggle } from "./modules/useModuleToggle";

/** Decorative English line under the title (kept in English in every language). */
const EN_LINE: Record<ModuleId, string> = {
  taskbar: "CUSTOM TASKBAR · LIVE EDITOR",
  wallpaper: "LIVE WALLPAPER",
  cursor: "CURSOR SCHEMES",
  labels: "DESKTOP LABELS",
  features: "HIDDEN FEATURES",
};

export function ModuleView({ id }: { id: ModuleId }) {
  const { t, tt } = useT();
  const { module } = useApp();
  const info = module(id);
  const { request, guardNode } = useModuleToggle();
  if (!info) return null;
  const unusable = info.availability.kind === "unsupported";
  const reason = info.availability.kind === "available" ? null : tt(info.availability.reason);
  const Body = unusable ? undefined : MODULE_BODIES[id];
  const toggle = (want: boolean) => request(id, want);
  return (
    <>
      {guardNode}
      <PageHeader
        title={t(`module.${id}.name`)}
        en={EN_LINE[id]}
        toggle={{ on: info.enabled, disabled: unusable, reason: reason ?? undefined, onChange: toggle }}
        right={<ProfileBar module={id} disabled={unusable} />}
      />
      {Body ? (
        <>
          {reason && (
            <div className="notice a" style={{ ["--d" as string]: ".22s" }}>
              <b>{reason}</b>
            </div>
          )}
          <Body info={info} />
        </>
      ) : (
      <div className="mod-body">
        {reason && (
          <div className="notice a" style={{ ["--d" as string]: ".25s" }}>
            <b>{reason}</b>
            {t(`module.${id}.desc`)}
          </div>
        )}
        <Skeleton rows={4} />
      </div>
      )}
    </>
  );
}
