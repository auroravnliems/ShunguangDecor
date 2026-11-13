"use client";
// Tools tab: export / import a file, fix priorities (/fixpriority), view usage subscriptions + "report usage".
// There is no "Fix LKG": ViVeTool's /fixlkg actually does nothing, so the core dropped it.
import { useEffect, useState } from "react";
import { Group, IconButton, Row } from "@/components/Group";
import { JoinButton } from "@/components/JoinButton";
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import type { FeatureImportReport, FeatureSubscription } from "@/lib/types";
import type { useFeatureWrite } from "./useWrite";

export function ToolsTab(p: { write: ReturnType<typeof useFeatureWrite> }) {
  const { t, tt } = useT();
  const { toast } = useApp();
  const { run, busy } = p.write;
  const [exported, setExported] = useState<string | null>(null);
  const [report, setReport] = useState<FeatureImportReport | null>(null);
  const [subs, setSubs] = useState<FeatureSubscription[] | null>(null);

  useEffect(() => {
    api
      .featuresSubscriptions()
      .then(setSubs)
      .catch((e) => {
        setSubs([]);
        toast("err", api.errorText(e, tt));
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const doExport = async () => {
    try {
      const dest = await api.pickSavePath("shunguangdecor-features.json", "json", t("features.tools.json_filter"));
      if (!dest) return;
      setExported(await api.featuresExport(dest));
    } catch (e) {
      toast("err", api.errorText(e, tt));
    }
  };
  const doImport = async () => {
    let src: string | null;
    try {
      src = await api.pickOpenPath("json", t("features.tools.json_filter"));
    } catch (e) {
      toast("err", api.errorText(e, tt));
      return;
    }
    if (!src) return;
    const file = src;
    const r = await run(() => api.featuresImport(file));
    if (r) setReport(r);
  };
  const doFix = async () => {
    const n = await run(() => api.featuresFixPriority());
    if (n !== undefined) toast("ok", t(n === 0 ? "features.tools.fixprio_none" : "features.tools.fixprio_done", { n }));
  };
  const notify = async (s: FeatureSubscription) => {
    // Through the shared write gate: first-time warning + a gentle "permission not granted" when No is clicked in the UAC prompt.
    const done = await run(() =>
      api.featuresNotifyUsage({ id: s.id, reporting_kind: s.reporting_kind, reporting_options: s.reporting_options }).then(() => true),
    );
    if (done) toast("ok", t("features.tools.notified", { id: s.id }));
  };

  return (
    <>
      <Group title={t("features.tools.export")} en="EXPORT">
        <p className="lb-hint">{t("features.tools.export_hint")}</p>
        <Row>
          <IconButton label={t("features.tools.export_btn")} onClick={() => void doExport()} />
          {exported && <span className="ft-path">{t("features.tools.exported", { path: exported })}</span>}
        </Row>
      </Group>
      <Group title={t("features.tools.import")} en="IMPORT">
        <p className="lb-hint">{t("features.tools.import_hint")}</p>
        <Row>
          <IconButton label={t("features.tools.import_btn")} disabled={busy} onClick={() => void doImport()} />
          {report && <span className="ft-path">{t("features.tools.imported", { applied: report.applied, skipped: report.skipped })}</span>}
        </Row>
        {report && report.warnings.length > 0 && (
          <ul className="ft-warn">
            {report.warnings.map((w, i) => (
              <li key={i}>{w}</li>
            ))}
          </ul>
        )}
      </Group>
      <Group title={t("features.tools.fixprio")} en="PRIORITY">
        <p className="lb-hint">{t("features.tools.fixprio_hint")}</p>
        <Row>
          <JoinButton label={t("features.tools.fixprio_btn")} variant="ghost" size="small" disabled={busy} onClick={() => void doFix()} />
        </Row>
      </Group>
      <Group title={t("features.tools.subs")} en="SUBSCRIPTIONS">
        <p className="lb-hint">{t("features.tools.subs_hint")}</p>
        {subs && subs.length === 0 && <p className="lb-hint">{t("features.tools.subs_empty")}</p>}
        {subs && subs.length > 0 && (
          <div className="ft-table-wrap">
            <table className="ft-table">
              <thead>
                <tr>
                  <th>ID</th>
                  <th>{t("features.tools.subs_kind")}</th>
                  <th>{t("features.tools.subs_options")}</th>
                  <th>{t("features.tools.subs_target")}</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {subs.map((s, i) => (
                  <tr key={`${s.id}-${i}`}>
                    <td>{s.id}</td>
                    <td>{s.reporting_kind}</td>
                    <td>{s.reporting_options}</td>
                    {/* Rust sends the u64 as a hex string so JS doesn't round it. */}
                    <td>{s.reporting_target}</td>
                    <td>
                      <IconButton label={t("features.tools.notify")} onClick={() => void notify(s)} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Group>
    </>
  );
}
