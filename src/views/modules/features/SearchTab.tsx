"use client";
// Search tab: search by ID or name, pick a row, then edit it in the detail pane (FeatureDetail).
import { useEffect, useRef, useState } from "react";
import { Group, IconButton, Row } from "@/components/Group";
import * as api from "@/lib/api";
import { parseFeatureQuery, stateLabelKey } from "@/lib/features";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import type { FeatureRow } from "@/lib/types";
import { FeatureDetail } from "./FeatureDetail";
import type { useFeatureWrite } from "./useWrite";

const LIMIT = 100;

/** Searches by the input box: if it is an ID, the `features_get` result is added too (so an ID with no name in the dictionary can still be opened). */
async function search(query: string): Promise<FeatureRow[]> {
  const q = parseFeatureQuery(query);
  if (q.kind === "empty") return [];
  const rows = await api.featuresSearch(query.trim(), LIMIT);
  if (q.kind === "id" && !rows.some((r) => r.id === q.id)) return [await api.featuresGet(q.id), ...rows];
  return rows;
}


export function SearchTab(p: { rev: number; write: ReturnType<typeof useFeatureWrite> }) {
  const { t, tt } = useT();
  const { toast } = useApp();
  const [query, setQuery] = useState("");
  const [rows, setRows] = useState<FeatureRow[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [searched, setSearched] = useState(false);
  /** The latest search; a late-arriving result from an older search is discarded. */
  const seq = useRef(0);

  useEffect(() => {
    const mine = ++seq.current;
    if (parseFeatureQuery(query).kind === "empty") {
      setRows([]);
      setSearched(false);
      return;
    }
    const timer = setTimeout(() => {
      search(query)
        .then((r) => {
          if (mine !== seq.current) return;
          setRows(r);
          setSearched(true);
        })
        .catch((e) => mine === seq.current && toast("err", api.errorText(e, tt)));
    }, 250);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, p.rev]);

  const row = rows.find((r) => r.id === selected) ?? null;
  const update = (r: FeatureRow) => setRows((rs) => rs.map((x) => (x.id === r.id ? r : x)));

  return (
    <>
      <Group title={t("features.tab.search")} en="SEARCH">
        <Row>
          <input
            className="wp-input"
            value={query}
            placeholder={t("features.search.placeholder")}
            aria-label={t("features.search.placeholder")}
            spellCheck={false}
            onChange={(e) => setQuery(e.target.value)}
          />
          {query && <IconButton label={t("features.search.clear")} onClick={() => setQuery("")} />}
        </Row>
        <p className="lb-hint">{t("features.search.hint")}</p>
        {searched && rows.length === 0 && <p className="lb-hint">{t("features.search.empty")}</p>}
        {rows.length > 0 && (
          <div className="ft-table-wrap">
            <table className="ft-table">
              <thead>
                <tr>
                  <th>ID</th>
                  <th>{t("features.col.name")}</th>
                  <th>{t("features.col.runtime")}</th>
                  <th>{t("features.col.boot")}</th>
                  <th>{t("features.col.changed")}</th>
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
                    <td>{r.name ?? <i>{t("features.row.unnamed")}</i>}</td>
                    <td>{t(stateLabelKey(r.runtime?.state ?? "default"))}</td>
                    <td>{t(stateLabelKey(r.boot?.state ?? "default"))}</td>
                    <td>{r.changedByApp ? t("features.val.yes") : ""}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Group>
      {row ? (
        <FeatureDetail key={row.id} row={row} write={p.write} onUpdated={update} />
      ) : (
        rows.length > 0 && <p className="lb-hint">{t("features.detail.pick")}</p>
      )}
    </>
  );
}
