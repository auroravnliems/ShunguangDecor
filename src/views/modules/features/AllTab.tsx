"use client";
// "All" tab: browse every feature (dictionary ∪ the machine's configuration), quick filter, sort, 50 rows per page.
// Microsoft has no documentation for individual IDs, so each row has a "Search the web" button (Rust builds the URL, the UI only sends the ID).
import { useCallback, useEffect, useRef, useState } from "react";
import { Group, IconButton, Row } from "@/components/Group";
import { Segmented } from "@/components/Segmented";
import { Toggle } from "@/components/Toggle";
import * as api from "@/lib/api";
import { ALL_PAGE_SIZE, pageCount, parsePageInput, stateLabelKey } from "@/lib/features";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import type { FeaturePage, FeatureRow } from "@/lib/types";
import { FeatureDetail } from "./FeatureDetail";
import type { useFeatureWrite } from "./useWrite";

type Sort = "name" | "id";
const DEBOUNCE_MS = 300;

function Pager(p: { page: number; pages: number; busy: boolean; onGo: (page: number) => void; top?: boolean }) {
  const { t } = useT();
  const [draft, setDraft] = useState("");
  const jump = () => {
    const target = parsePageInput(draft, p.pages);
    setDraft("");
    if (target !== null && target !== p.page) p.onGo(target);
  };
  const first = p.page <= 0;
  const last = p.page >= p.pages - 1;
  return (
    <div className={`ft-pager${p.top ? " top" : ""}`}>
      <IconButton label="«" title={t("features.all.first")} disabled={first || p.busy} onClick={() => p.onGo(0)} />
      <IconButton label="‹" title={t("features.all.prev")} disabled={first || p.busy} onClick={() => p.onGo(p.page - 1)} />
      <span className="ft-pager-info">{t("features.all.page_of", { page: p.page + 1, pages: p.pages })}</span>
      <IconButton label="›" title={t("features.all.next")} disabled={last || p.busy} onClick={() => p.onGo(p.page + 1)} />
      <IconButton label="»" title={t("features.all.last")} disabled={last || p.busy} onClick={() => p.onGo(p.pages - 1)} />
      <input
        className="wp-input ft-num ft-pager-jump"
        inputMode="numeric"
        value={draft}
        placeholder={t("features.all.jump")}
        aria-label={t("features.all.jump")}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && jump()}
      />
    </div>
  );
}

export function AllTab(p: { rev: number; write: ReturnType<typeof useFeatureWrite> }) {
  const { t, tt } = useT();
  const { toast } = useApp();
  const [text, setText] = useState("");
  /** The filter box value after debouncing — only changing this reloads the list. */
  const [appliedText, setAppliedText] = useState("");
  const [configuredOnly, setConfiguredOnly] = useState(false);
  const [hideUnnamed, setHideUnnamed] = useState(false);
  const [sort, setSort] = useState<Sort>("name");
  const [page, setPage] = useState(0);
  const [data, setData] = useState<FeaturePage | null>(null);
  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<number | null>(null);
  /** The latest load; a late result from an older load is discarded. */
  const seq = useRef(0);

  // Filter box: wait until typing stops before applying it, and always go back to the first page.
  useEffect(() => {
    if (text === appliedText) return;
    const timer = setTimeout(() => {
      setAppliedText(text);
      setPage(0);
    }, DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [text, appliedText]);

  const load = useCallback(() => {
    const mine = ++seq.current;
    setLoading(true);
    api
      .featuresList({ page, pageSize: ALL_PAGE_SIZE, configuredOnly, hideUnnamed, text: appliedText, sort })
      .then((r) => {
        if (mine !== seq.current) return;
        setData(r);
        // Rust clamps an out-of-range page to the last page: follow the real page so the navigation buttons stay in sync.
        if (r.page !== page) setPage(r.page);
      })
      .catch((e) => mine === seq.current && toast("err", api.errorText(e, tt)))
      .finally(() => mine === seq.current && setLoading(false));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page, configuredOnly, hideUnnamed, appliedText, sort]);

  // Reload when the filter/page changes, and after every write (p.rev) while keeping the current page.
  useEffect(() => {
    load();
  }, [load, p.rev]);

  const change = <T,>(set: (v: T) => void) => (v: T) => {
    set(v);
    setPage(0);
  };

  const rows = data?.rows ?? [];
  const row = rows.find((r) => r.id === selected) ?? null;
  const update = (r: FeatureRow) => setData((d) => d && { ...d, rows: d.rows.map((x) => (x.id === r.id ? r : x)) });
  const webSearch = (id: number) => api.featuresWebSearch(id).catch((e) => toast("err", api.errorText(e, tt)));
  const pages = pageCount(data?.total ?? 0, ALL_PAGE_SIZE);

  return (
    <>
      <Group title={t("features.tab.all")} en="ALL">
        <p className="lb-hint">{t("features.all.note")}</p>
        <Row>
          <input
            className="wp-input"
            value={text}
            placeholder={t("features.all.filter")}
            aria-label={t("features.all.filter")}
            spellCheck={false}
            onChange={(e) => setText(e.target.value)}
          />
          {text && <IconButton label={t("features.search.clear")} onClick={() => setText("")} />}
        </Row>
        <div className="ft-filters">
          <label className="ft-check">
            <Toggle on={configuredOnly} onChange={change(setConfiguredOnly)} label={t("features.all.configured_only")} />
            <span>{t("features.all.configured_only")}</span>
          </label>
          <label className="ft-check">
            <Toggle on={hideUnnamed} onChange={change(setHideUnnamed)} label={t("features.all.hide_unnamed")} />
            <span>{t("features.all.hide_unnamed")}</span>
          </label>
        </div>
        <Row label={t("features.all.sort")}>
          <Segmented<Sort>
            options={[
              { value: "name", label: t("features.all.sort_name") },
              { value: "id", label: t("features.all.sort_id") },
            ]}
            value={sort}
            onChange={change(setSort)}
          />
        </Row>
        {data && <p className="lb-hint ft-count">{t("features.all.count", { n: data.total })}</p>}
        {data && data.total === 0 && <p className="lb-hint">{t("features.search.empty")}</p>}
        {data && data.total > 0 && <Pager top page={data.page} pages={pages} busy={loading} onGo={setPage} />}
        {rows.length > 0 && (
          <div className={`ft-table-wrap ft-all-wrap${loading ? " busy" : ""}`}>
            <table className="ft-table">
              <thead>
                <tr>
                  <th>ID</th>
                  <th>{t("features.col.name")}</th>
                  <th>{t("features.col.runtime")}</th>
                  <th>{t("features.col.boot")}</th>
                  <th>{t("features.col.changed")}</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {rows.map((r) => (
                  <tr key={r.id} className={r.id === selected ? "on" : undefined} onClick={() => setSelected(r.id)}>
                    <td>
                      <button className="ft-link" onClick={() => setSelected(r.id)}>
                        {r.id}
                      </button>
                    </td>
                    <td>
                      {r.name ?? (
                        <>
                          —<span className="ft-dim"> {t("features.all.unnamed_note")}</span>
                        </>
                      )}
                    </td>
                    <td>{t(stateLabelKey(r.runtime?.state ?? "default"))}</td>
                    <td>{t(stateLabelKey(r.boot?.state ?? "default"))}</td>
                    <td>{r.changedByApp ? t("features.val.yes") : ""}</td>
                    <td>
                      {/* Stop propagation: clicking this button does not select the row. */}
                      <button
                        className="ft-web"
                        onClick={(e) => {
                          e.stopPropagation();
                          void webSearch(r.id);
                        }}
                      >
                        {t("features.all.web")}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        {data && data.total > 0 && <Pager page={data.page} pages={pages} busy={loading} onGo={setPage} />}
      </Group>
      {row ? (
        <FeatureDetail key={row.id} row={row} write={p.write} onUpdated={update} />
      ) : (
        rows.length > 0 && <p className="lb-hint">{t("features.detail.pick")}</p>
      )}
    </>
  );
}
