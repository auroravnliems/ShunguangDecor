"use client";
// Card for one part on the Home page (v9 .mc). Clicking the card opens the part's page; its own toggle does not open the page.
import { useT } from "@/lib/i18n";
import type { ModuleInfo } from "@/lib/types";
import { Toggle } from "./Toggle";

export function ModuleCard({
  info,
  index,
  onOpen,
  onToggle,
}: {
  info: ModuleInfo;
  index: number;
  onOpen: () => void;
  onToggle: (on: boolean) => void;
}) {
  const { t, tt } = useT();
  const name = t(`module.${info.id}.name`);
  const reason = info.availability.kind === "available" ? null : tt(info.availability.reason);
  const unusable = info.availability.kind === "unsupported";
  const sub = info.fault
    ? t("module.state.faulted")
    : info.status.summary
      ? tt(info.status.summary)
      : (reason ?? (info.enabled ? t("module.state.on") : t("module.state.off")));
  const cls = ["mc", "a", "ac", info.enabled ? "on" : "", info.fault ? "fault" : ""].filter(Boolean).join(" ");
  return (
    <div
      className={cls}
      role="button"
      tabIndex={0}
      style={{ ["--d" as string]: `${0.55 + index * 0.1}s` }}
      onClick={onOpen}
      onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && e.target === e.currentTarget && onOpen()}
      title={name}
    >
      <div className="face">
        <h4>{name}</h4>
        <p title={sub}>{sub}</p>
      </div>
      {info.status.details.length > 0 ? (
        <div className="more">
          {info.status.details.slice(0, 4).map(([k, v], i) => (
            <div key={i}>
              <b>{tt(k)}</b>
              {tt(v)}
            </div>
          ))}
        </div>
      ) : (
        <div className="more desc">{t(`module.${info.id}.desc`)}</div>
      )}
      <Toggle on={info.enabled} disabled={unusable} title={reason ?? name} label={name} onChange={onToggle} />
      <div className="num">
        <em>›››</em>
        {String(info.number).padStart(2, "0")}
      </div>
    </div>
  );
}
