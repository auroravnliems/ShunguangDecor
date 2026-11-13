"use client";
// Cursors page: cursor sets, details of the 17 roles, size & scaling style.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { CursorSprite, useFrameIndex } from "@/components/CursorSprite";
import { IconButton } from "@/components/Group";
import { JoinButton } from "@/components/JoinButton";
import { PromptDialog } from "@/components/PromptDialog";
import { RangeSlider } from "@/components/RangeSlider";
import { Segmented } from "@/components/Segmented";
import { SubTabs, TabPanel } from "@/components/SubTabs";
import * as api from "@/lib/api";
import {
  type CursorConfig,
  cursorImageUrl,
  importIntent,
  MAX_SIZE,
  MIN_SIZE,
  normalizeCursorConfig,
  type Role,
  ROLES,
  type RoleInfo,
  type SchemeDetail,
  type SchemeSummary,
  SIZE_STEP,
} from "@/lib/cursor";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import type { WallpaperView } from "@/lib/types";
import { useModuleConfig } from "@/lib/useModuleConfig";
import type { ModuleBodyProps } from "./index";

type Tab = "schemes" | "detail" | "size";
type Bg = "light" | "dark" | "wall";
type Ask =
  | { kind: "rename" }
  | { kind: "delete" }
  | { kind: "role"; role: Role }
  | null;

/** Sample image when running in a browser (no Tauri): a simple arrow. */
const MOCK_IMG =
  "data:image/svg+xml;utf8," +
  encodeURIComponent(
    '<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><path d="M3 2 L3 24 L9 18 L13 28 L17 26 L13 17 L21 17 Z" fill="#fff" stroke="#22262f" stroke-width="2" stroke-linejoin="round"/></svg>',
  );

const PREVIEW_ROLES: Role[] = ["Arrow", "Hand", "IBeam", "Wait"];

function useImageUrls() {
  const { info } = useApp();
  const base = info?.protocolBase ?? "";
  return useCallback(
    (id: string, role: Role, which: "sheet" | number, v: number) =>
      api.isTauri() ? cursorImageUrl(base, id, role, which, v) : MOCK_IMG,
    [base],
  );
}

function RoleSprite(p: { detail: SchemeDetail | undefined; role: Role; size: number; v: number; pixelated?: boolean }) {
  const url = useImageUrls();
  const info = p.detail?.roles[p.role];
  if (!p.detail || !info) return <span className="cur-sprite empty" style={{ width: p.size, height: p.size }} />;
  return (
    <CursorSprite
      url={url(p.detail.id, p.role, "sheet", p.v)}
      frames={info.frames}
      delaysMs={info.delaysMs}
      width={info.width}
      height={info.height}
      size={p.size}
      pixelated={p.pixelated}
    />
  );
}

function RolePicker(p: {
  role: Role;
  detail: SchemeDetail;
  onPick: (file: string | null) => void;
  onCancel: () => void;
}) {
  const { t } = useT();
  const current = p.detail.roles[p.role]?.file;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && p.onCancel();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [p]);
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && p.onCancel()}>
      <div className="dialog cur-picker" role="dialog" aria-modal="true">
        <h3>{t("cursor.role_pick_title", { role: t(`cursor.role.${p.role}`) })}</h3>
        <div className="cur-files">
          <button className={!current ? "on" : undefined} onClick={() => p.onPick(null)}>
            {t("cursor.role_pick_empty")}
          </button>
          {p.detail.files.map((f) => (
            <button key={f} className={f === current ? "on" : undefined} onClick={() => p.onPick(f)} title={f}>
              {f}
            </button>
          ))}
        </div>
        <div className="actions">
          <button className="jb ghost small" onClick={p.onCancel}>
            <b>{t("dialog.cancel")}</b>
          </button>
        </div>
      </div>
    </div>
  );
}

/** Three hover-test boxes: normal pointer, hand, text I-beam (an animated set changes frames over time). */
function TryZone({ detail, v }: { detail: SchemeDetail; v: number }) {
  const { t } = useT();
  const url = useImageUrls();
  const cursorOf = (role: Role, idx: number): string => {
    const r = detail.roles[role];
    if (!r || !api.isTauri() || r.width > 128 || r.height > 128) return "auto";
    const n = Math.min(idx, r.frames - 1);
    return `url("${url(detail.id, role, n, v)}") ${r.hotspot[0]} ${r.hotspot[1]}, auto`;
  };
  const a = useFrameIndex(detail.roles.Arrow?.delaysMs ?? []);
  const h = useFrameIndex(detail.roles.Hand?.delaysMs ?? []);
  const i = useFrameIndex(detail.roles.IBeam?.delaysMs ?? []);
  return (
    <div className="cur-try a" style={{ ["--d" as string]: ".4s" }}>
      <div style={{ cursor: cursorOf("Arrow", a) }}>
        {t("cursor.try.normal")}
        <small>{t("cursor.try.normal_sub")}</small>
      </div>
      <div style={{ cursor: cursorOf("Hand", h) }}>
        <span className="lnk">{t("cursor.try.link")}</span>
        <small>{t("cursor.try.link_sub")}</small>
      </div>
      <div>
        <input placeholder={t("cursor.try.text_placeholder")} style={{ cursor: cursorOf("IBeam", i) }} />
        <small>{t("cursor.try.text_sub")}</small>
      </div>
    </div>
  );
}

export function CursorView({ info }: ModuleBodyProps) {
  const { t, tt } = useT();
  const { toast, applyModule } = useApp();
  const { cfg, update, flush } = useModuleConfig<CursorConfig>("cursor", normalizeCursorConfig);
  const imgUrl = useImageUrls();

  const [tab, setTab] = useState<Tab>("schemes");
  const [list, setList] = useState<SchemeSummary[]>([]);
  const [details, setDetails] = useState<Record<string, SchemeDetail>>({});
  const [viewId, setViewId] = useState<string | null>(null);
  const [bg, setBg] = useState<Bg>("light");
  const [wall, setWall] = useState<WallpaperView | null>(null);
  const [busy, setBusy] = useState(false);
  const [dropping, setDropping] = useState(false);
  const [ask, setAsk] = useState<Ask>(null);
  const [v, setV] = useState(0);
  const fail = useCallback((e: unknown) => toast("err", api.errorText(e, tt)), [toast, tt]);

  const activeId = info.enabled ? (cfg?.scheme ?? null) : null;

  // --- library ---
  const reload = useCallback(() => {
    api
      .cursorList()
      .then((l) => {
        setList(l);
        setDetails({});
        setV((x) => x + 1);
      })
      .catch(fail);
  }, [fail]);
  useEffect(() => {
    reload();
    const off = api.onCursorLibraryChanged(reload);
    return () => {
      off.then((f) => f());
    };
  }, [reload]);

  // Load the details of each set (the cards need them to draw the real cursor).
  useEffect(() => {
    const missing = list.filter((s) => !details[s.id]);
    if (missing.length === 0) return;
    let alive = true;
    Promise.all(missing.map((s) => api.cursorDetail(s.id).catch(() => null))).then((ds) => {
      if (!alive) return;
      setDetails((cur) => {
        const next = { ...cur };
        for (const d of ds) if (d) next[d.id] = d;
        return next;
      });
    });
    return () => {
      alive = false;
    };
  }, [list, details]);

  // The set shown in detail: defaults to the set in use, then the first set.
  useEffect(() => {
    if (viewId && list.some((s) => s.id === viewId)) return;
    const pick = (cfg?.scheme && list.some((s) => s.id === cfg.scheme) ? cfg.scheme : null) ?? list[0]?.id ?? null;
    setViewId(pick);
  }, [list, viewId, cfg?.scheme]);

  useEffect(() => {
    api
      .getCurrentWallpaper()
      .then(setWall)
      .catch(() => setWall(null));
  }, []);

  // --- import ---
  const importPaths = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) return;
      setBusy(true);
      let total = 0;
      let first: string | null = null;
      for (const p of paths) {
        try {
          const ids = await api.cursorImport(p);
          total += ids.length;
          first ??= ids[0] ?? null;
        } catch (e) {
          const name = p.split(/[\\/]/).pop() ?? p;
          toast("err", `${name}: ${api.errorText(e, tt)}`);
        }
      }
      setBusy(false);
      if (total > 0) {
        toast("ok", t("cursor.toast.imported", { count: total }));
        if (first) setViewId(first);
        reload();
      }
    },
    [reload, t, toast, tt],
  );
  const importFile = useCallback(async () => {
    const p = await api.pickCursorFile(t("cursor.import_filter"));
    if (p) await importPaths([p]);
  }, [importPaths, t]);
  const importFolder = useCallback(async () => {
    const p = await api.pickFolder();
    if (p) await importPaths([p]);
  }, [importPaths]);

  // Coming from Home via "Import cursors": open the file picker right away.
  useEffect(() => {
    if (importIntent.pending) {
      importIntent.pending = false;
      void importFile();
    }
  }, [importFile]);

  // Drag and drop files/folders onto the whole page.
  const dropRef = useRef(importPaths);
  dropRef.current = importPaths;
  useEffect(() => {
    const off = api.onFileDrop(
      (paths) => void dropRef.current(paths),
      (over) => setDropping(over),
    );
    return () => {
      off.then((f) => f());
    };
  }, []);

  // --- select / restore ---
  const choose = async (s: SchemeSummary) => {
    if (!cfg) return;
    update({ scheme: s.id });
    if (!info.enabled) {
      await flush();
      try {
        applyModule(await api.setModuleEnabled("cursor", true));
        toast("ok", t("cursor.toast.using", { name: s.name }));
      } catch (e) {
        fail(e);
      }
    } else {
      toast("ok", t("cursor.toast.using", { name: s.name }));
    }
  };
  const restore = async () => {
    try {
      applyModule(await api.setModuleEnabled("cursor", false));
      toast("ok", t("cursor.toast.restored"));
    } catch (e) {
      fail(e);
    }
  };

  const viewed = viewId ? details[viewId] : undefined;
  const viewedName = list.find((s) => s.id === viewId)?.name ?? "";
  const previewId = activeId ?? viewId;
  const previewDetail = previewId ? details[previewId] : undefined;
  const arrow: RoleInfo | undefined = previewDetail?.roles.Arrow;
  const sizeLong = cfg?.size ?? MIN_SIZE;

  const tabs = useMemo(
    () => [
      { id: "schemes" as const, label: t("cursor.tab.schemes") },
      { id: "detail" as const, label: t("cursor.tab.detail") },
      { id: "size" as const, label: t("cursor.tab.size") },
    ],
    [t],
  );

  const kindText = (d: SchemeDetail | undefined): string => {
    const frames = d?.roles.Arrow?.frames ?? 1;
    return frames > 1 ? t("cursor.kind.animated", { frames }) : t("cursor.kind.static");
  };

  const bgStyle =
    bg === "wall" && wall ? { backgroundImage: `linear-gradient(rgba(0,0,0,.12), rgba(0,0,0,.12)), url("${wall.url}")` } : undefined;

  return (
    <div className="cur-body">
      <div className="cur-top a" style={{ ["--d" as string]: ".2s" }}>
        <SubTabs tabs={tabs} value={tab} onChange={setTab} />
        <div className="cur-actions">
          <IconButton label={t("cursor.restore")} onClick={() => void restore()} disabled={!info.enabled} />
          <JoinButton label={t("cursor.import")} sub="IMPORT" size="small" disabled={busy} onClick={() => void importFile()} />
        </div>
      </div>

      <div className="cur-main">
        <TabPanel tab={tab}>
          {tab === "schemes" && (
            <div className="cur-grid">
              {list.length === 0 && <div className="cur-empty">{t("cursor.empty")}</div>}
              {list.map((s, i) => {
                const d = details[s.id];
                const on = s.id === activeId;
                return (
                  <div
                    key={s.id}
                    className={`cur-card${on ? " on" : ""}`}
                    role="button"
                    tabIndex={0}
                    style={{ animationDelay: `${i * 0.04}s` }}
                    onClick={() => void choose(s)}
                    onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && void choose(s)}
                  >
                    <div className="ims">
                      {PREVIEW_ROLES.map((r) => (
                        <RoleSprite key={r} detail={d} role={r} size={34} v={v} />
                      ))}
                    </div>
                    <div className="nm" title={s.name}>
                      {s.name}
                      <small>
                        {t("cursor.meta", { count: s.roleCount, kind: kindText(d) })}
                      </small>
                    </div>
                    {on && <div className="use">{t("cursor.in_use")}</div>}
                    <button
                      className="more"
                      onClick={(e) => {
                        e.stopPropagation();
                        setViewId(s.id);
                        setTab("detail");
                      }}
                    >
                      {t("cursor.view_detail")}
                    </button>
                  </div>
                );
              })}
              <div
                className="cur-card add"
                role="button"
                tabIndex={0}
                onClick={() => !busy && void importFile()}
                onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && !busy && void importFile()}
              >
                <b>{busy ? t("cursor.importing") : `+ ${t("cursor.import_card")}`}</b>
                <small>{t("cursor.import_hint")}</small>
                <button
                  className="more"
                  disabled={busy}
                  onClick={(e) => {
                    e.stopPropagation();
                    void importFolder();
                  }}
                >
                  {t("cursor.import_folder")}
                </button>
              </div>
            </div>
          )}

          {tab === "detail" &&
            (viewed ? (
              <div className="cur-detail">
                <div className="cur-dbar">
                  <div className="nm">
                    {viewedName}
                    <small>{t("cursor.role_count", { count: Object.keys(viewed.roles).length })}</small>
                  </div>
                  <Segmented<Bg>
                    options={[
                      { value: "light", label: t("cursor.bg.light") },
                      { value: "dark", label: t("cursor.bg.dark") },
                      { value: "wall", label: t("cursor.bg.wall") },
                    ]}
                    value={bg}
                    onChange={setBg}
                  />
                  <IconButton label={t("cursor.rename")} onClick={() => setAsk({ kind: "rename" })} />
                  <IconButton label={t("cursor.delete")} onClick={() => setAsk({ kind: "delete" })} />
                </div>
                <div className={`cur-cgrid ${bg}`} style={bgStyle}>
                  {ROLES.map((r, k) => (
                    <button
                      key={r}
                      className="cell"
                      style={{ animationDelay: `${k * 0.03}s` }}
                      onClick={() => setAsk({ kind: "role", role: r })}
                      title={viewed.roles[r]?.file ?? t("cursor.role_empty")}
                    >
                      <RoleSprite detail={viewed} role={r} size={64} v={v} pixelated />
                      <span>{t(`cursor.role.${r}`)}</span>
                    </button>
                  ))}
                </div>
                <TryZone detail={viewed} v={v} />
              </div>
            ) : (
              <div className="cur-empty">{t("cursor.no_scheme_view")}</div>
            ))}

          {tab === "size" && cfg && (
            <div className="cur-size">
              <div className="grp">
                <h5>
                  {t("cursor.group.size")}
                  <small>CURSOR SIZE</small>
                </h5>
                <div className="row">
                  <label>{t("cursor.size")}</label>
                  <RangeSlider
                    min={MIN_SIZE}
                    max={MAX_SIZE}
                    step={SIZE_STEP}
                    value={cfg.size}
                    onChange={(size) => update({ size })}
                    format={(n) => t("cursor.px", { n })}
                    label={t("cursor.size")}
                  />
                </div>
                <div className="row">
                  <label>{t("cursor.scaling")}</label>
                  <Segmented
                    options={[
                      { value: "sharp" as const, label: t("cursor.scaling.sharp") },
                      { value: "smooth" as const, label: t("cursor.scaling.smooth") },
                    ]}
                    value={cfg.scaling}
                    onChange={(scaling) => update({ scaling })}
                  />
                  <span className="hint">{t("cursor.scaling_hint")}</span>
                </div>
                <div className="row">
                  <span className="hint">{t("cursor.size_dpi")}</span>
                </div>
              </div>
              <div className="cur-preview" style={wall ? { backgroundImage: `linear-gradient(rgba(0,0,0,.1), rgba(0,0,0,.1)), url("${wall.url}")` } : undefined}>
                {previewId && arrow ? (
                  <img
                    className={cfg.scaling === "sharp" ? "px" : undefined}
                    src={imgUrl(previewId, "Arrow", 0, v)}
                    alt=""
                    draggable={false}
                    style={{
                      width: (arrow.width / Math.max(arrow.width, arrow.height)) * sizeLong,
                      height: (arrow.height / Math.max(arrow.width, arrow.height)) * sizeLong,
                    }}
                  />
                ) : (
                  <span className="hint">{t("cursor.no_scheme_view")}</span>
                )}
                <small>{t("cursor.size_preview")}</small>
              </div>
            </div>
          )}
        </TabPanel>
      </div>

      <div className="cur-foot">
        <p>{t("cursor.game_note")}</p>
        <p>
          {t("cursor.adv_text")}{" "}
          <button onClick={() => void api.openDataDir().catch(fail)}>{t("cursor.adv_open")}</button>
        </p>
      </div>

      {dropping && <div className="cur-drop">{t("cursor.drop_here")}</div>}

      {ask?.kind === "rename" && viewId && (
        <PromptDialog
          title={t("cursor.rename_title")}
          label={t("cursor.rename_label")}
          initial={viewedName}
          maxLength={60}
          onCancel={() => setAsk(null)}
          onConfirm={(name) => {
            setAsk(null);
            api.cursorRename(viewId, name).catch(fail);
          }}
        />
      )}
      {ask?.kind === "delete" && viewId && (
        <ConfirmDialog
          title={t("cursor.delete_title")}
          message={t("cursor.delete_body", { name: viewedName })}
          confirmLabel={t("cursor.delete")}
          danger
          onCancel={() => setAsk(null)}
          onConfirm={() => {
            setAsk(null);
            api
              .cursorDelete(viewId)
              .then(() => {
                toast("ok", t("cursor.toast.deleted", { name: viewedName }));
                setViewId(null);
              })
              .catch(fail);
          }}
        />
      )}
      {ask?.kind === "role" && viewed && (
        <RolePicker
          role={ask.role}
          detail={viewed}
          onCancel={() => setAsk(null)}
          onPick={(file) => {
            setAsk(null);
            api.cursorSetRole(viewed.id, ask.role, file).catch(fail);
          }}
        />
      )}
    </div>
  );
}
