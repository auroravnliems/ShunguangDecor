"use client";
// Settings: language, start with Windows, reduce effects, export/import everything,
// restore defaults, data folder, bug report, about + credits.
import { useState } from "react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Group, IconButton, Row } from "@/components/Group";
import { JoinButton } from "@/components/JoinButton";
import { PageHeader } from "@/components/PageHeader";
import { SelectBox } from "@/components/SelectBox";
import { Toggle } from "@/components/Toggle";
import * as api from "@/lib/api";
import { stamp } from "@/lib/format";
import { useT } from "@/lib/i18n";
import { LANGS } from "@/lib/langs";
import { useApp } from "@/lib/store";

const CREDITS: [string, string][] = [
  ["Lively Wallpaper", "GPL-3.0"],
  ["TranslucentTB", "GPL-3.0"],
  ["ViVe", "GPL-3.0"],
  ["YASB", "MIT"],
];

export function SettingsView() {
  const { t, tt } = useT();
  const { info, settings, applySettings, applyModule, toast, setConfirmRestore } = useApp();
  const [importSrc, setImportSrc] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  if (!info || !settings) return null;
  const fail = (e: unknown) => toast("err", api.errorText(e, tt));

  const exportAll = async () => {
    const dest = await api.pickSavePath(`ShunguangDecor-${stamp(new Date())}.sgdecor`, "sgdecor", "ShunguangDecor");
    if (dest) api.exportAll(dest).then(() => toast("ok", t("toast.exported"))).catch(fail);
  };
  const pickImport = async () => {
    const src = await api.pickOpenPath("sgdecor", "ShunguangDecor");
    if (src) setImportSrc(src);
  };
  const runImport = async () => {
    if (!importSrc) return;
    setBusy(true);
    try {
      const r = await api.importAll(importSrc);
      toast("ok", t("toast.import_all_done", { count: r.files }));
      (await api.listModules()).forEach(applyModule);
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
      setImportSrc(null);
    }
  };
  const bugReport = async () => {
    const dest = await api.pickSavePath(`ShunguangDecor-baocao-${stamp(new Date())}.zip`, "zip", "Zip");
    if (dest) api.exportBugReport(dest).then(() => toast("ok", t("toast.report_saved"))).catch(fail);
  };
  const discord = () => api.openLink("discord").catch(fail);

  return (
    <>
      <PageHeader title={t("settings.title")} en="SETTINGS · PREFERENCES" />
      <div className="set-l">
        <div className="a" style={{ ["--d" as string]: ".2s" }}>
          <Group title={t("settings.general")} en="GENERAL">
            <Row label={t("settings.language")}>
              <SelectBox
                options={LANGS.map((l) => ({ value: l.code, label: l.native }))}
                value={settings.language}
                label={t("settings.language")}
                onChange={(code) => api.setLanguage(code).then(applySettings).catch(fail)}
              />
            </Row>
            <Row label={t("settings.autostart.windows")}>
              <Toggle on={settings.autostart} onChange={(on) => api.setAutostart(on).then(applySettings).catch(fail)} />
            </Row>
            <Row label={t("settings.reduce_motion")}>
              <Toggle on={settings.reduceMotion} onChange={(on) => api.setReduceMotion(on).then(applySettings).catch(fail)} />
            </Row>
            <Row label={t("settings.follow_os_motion.windows")}>
              <Toggle
                on={settings.followOsMotion}
                disabled={settings.reduceMotion}
                onChange={(on) => api.setFollowOsMotion(on).then(applySettings).catch(fail)}
              />
              <span className="hint">{t("settings.follow_os_motion_hint")}</span>
            </Row>
          </Group>
        </div>
        <div className="a" style={{ ["--d" as string]: ".28s" }}>
          <Group title={t("settings.config")} en="CONFIGURATION">
            <p className="set-text">{t("settings.config_text")}</p>
            <div className="btnrow">
              <IconButton label={t("settings.export_all")} onClick={exportAll} />
              <IconButton label={t("settings.import_all")} onClick={pickImport} />
              <IconButton label={t("settings.open_data")} onClick={() => api.openDataDir().catch(fail)} />
            </div>
          </Group>
        </div>
        <div className="a" style={{ ["--d" as string]: ".36s" }}>
          <Group title={t("settings.restore")} en="RESTORE">
            <p className="set-text">{t("settings.restore_text")}</p>
            <JoinButton label={t("settings.restore_btn")} variant="ghost" size="small" danger onClick={() => setConfirmRestore(true)} />
          </Group>
        </div>
      </div>
      <div className="set-r">
        <div className="a" style={{ ["--d" as string]: ".3s" }}>
          <Group title={t("settings.about")} en="ABOUT">
            <div className="about-head">
              <div className="ic" />
              <div>
                <b>ShunguangDecor</b>
                <span>
                  {t("settings.version", { version: info.version })} · Windows
                  {info.portable ? ` · ${t("settings.portable")}` : ""}
                </span>
              </div>
            </div>
            <p className="set-text">{t("settings.license", { license: "GPL-3.0-or-later" })}</p>
            <p className="set-text">{t("settings.credits")}</p>
            <ul className="credits">
              {CREDITS.map(([name, lic]) => (
                <li key={name}>
                  {name}
                  <small>{lic}</small>
                </li>
              ))}
            </ul>
            <div className="btnrow">
              <IconButton label="Discord" onClick={discord} />
              {info.githubUrl && <IconButton label="GitHub" onClick={() => api.openLink("github").catch(fail)} />}
            </div>
          </Group>
        </div>
        <div className="a" style={{ ["--d" as string]: ".38s" }}>
          <Group title={t("settings.support")} en="SUPPORT">
            <p className="set-text">{t("settings.report_text")}</p>
            <div className="btnrow">
              <IconButton label={t("settings.report_btn")} onClick={bugReport} />
              <IconButton label="Discord" onClick={discord} />
            </div>
          </Group>
        </div>
      </div>
      {importSrc && (
        <ConfirmDialog
          title={t("settings.import_title")}
          message={t("settings.import_body")}
          confirmLabel={t("settings.import_all")}
          busy={busy}
          onConfirm={runImport}
          onCancel={() => setImportSrc(null)}
        />
      )}
    </>
  );
}
