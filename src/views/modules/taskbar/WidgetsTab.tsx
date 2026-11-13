"use client";
// Widgets tab: 3 zones Left/Center/Right, drag and drop via Pointer Events (no library), add from the catalog, double-click to customize.
// Keyboard: arrow keys to reorder/move between zones, Enter to customize, Delete to remove.
import { type KeyboardEvent, type PointerEvent as ReactPointerEvent, useEffect, useRef, useState } from "react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Group, IconButton, Row } from "@/components/Group";
import { Segmented } from "@/components/Segmented";
import { Toggle } from "@/components/Toggle";
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";
import {
  addWidget,
  clockLabel,
  clockOptions,
  kindOf,
  moveWidget,
  optionsOf,
  removeWidget,
  setWidgetOptions,
  WIDGET_KINDS,
  type WidgetSlot,
  ZONE_NAMES,
  type ZoneName,
  type Zones,
} from "@/lib/taskbar";
import type { WidgetCatalogItem } from "@/lib/types";
import type { TabProps } from "./tabProps";

/** Only dragging beyond this many px counts as a drag (below the threshold it's a click). */
const DRAG_THRESHOLD = 4;

interface Drag {
  from: WidgetSlot;
  kind: string;
  startX: number;
  startY: number;
  x: number;
  y: number;
  moved: boolean;
  to: WidgetSlot | null;
}

const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const HAS_OPTIONS = new Set(["clock", "search", "apps"]);

function OptionsDialog(p: { kind: string; initial: Record<string, unknown>; onSave: (o: Record<string, unknown>) => void; onCancel: () => void }) {
  const { t } = useT();
  const [opts, setOpts] = useState(p.initial);
  const name = t(`taskbar.widget.${p.kind}.name`);
  const without = (key: string) => {
    const { [key]: _drop, ...rest } = opts;
    return rest;
  };
  let body: React.ReactNode;
  if (p.kind === "clock") {
    const c = clockOptions(opts);
    const set = (next: { hour12: boolean; seconds: boolean }) =>
      setOpts(next.hour12 || next.seconds ? { ...opts, label: clockLabel(next) } : without("label"));
    body = (
      <>
        <Row>
          <Toggle on={c.hour12} onChange={(hour12) => set({ ...c, hour12 })} label={t("taskbar.opt.clock.hour12")} />
          <span className="lb-rule">{t("taskbar.opt.clock.hour12")}</span>
        </Row>
        <Row>
          <Toggle on={c.seconds} onChange={(seconds) => set({ ...c, seconds })} label={t("taskbar.opt.clock.seconds")} />
          <span className="lb-rule">{t("taskbar.opt.clock.seconds")}</span>
        </Row>
      </>
    );
  } else if (p.kind === "search") {
    const on = opts.label === true;
    body = (
      <Row>
        <Toggle on={on} onChange={(v) => setOpts(v ? { ...opts, label: true } : without("label"))} label={t("taskbar.opt.search.label")} />
        <span className="lb-rule">{t("taskbar.opt.search.label")}</span>
      </Row>
    );
  } else if (p.kind === "apps") {
    const tl = isObj(opts.title_label) ? opts.title_label : {};
    const on = tl.enabled === true;
    body = (
      <Row>
        <Toggle
          on={on}
          onChange={(v) => {
            if (v) setOpts({ ...opts, title_label: { ...tl, enabled: true } });
            else {
              const { enabled: _e, ...rest } = tl;
              setOpts(Object.keys(rest).length ? { ...opts, title_label: rest } : without("title_label"));
            }
          }}
          label={t("taskbar.opt.apps.title")}
        />
        <span className="lb-rule">{t("taskbar.opt.apps.title")}</span>
      </Row>
    );
  } else {
    body = <span className="lb-rule">{t("taskbar.widgets.no_options")}</span>;
  }
  const none = !HAS_OPTIONS.has(p.kind);
  return (
    <ConfirmDialog
      title={t("taskbar.widgets.options", { name })}
      message={<div className="tb-opts">{body}</div>}
      confirmLabel={none ? undefined : t("taskbar.widgets.save")}
      noCancel={none}
      onConfirm={() => (none ? p.onCancel() : p.onSave(opts))}
      onCancel={p.onCancel}
    />
  );
}

export function WidgetsTab({ cfg, set }: TabProps) {
  const { t } = useT();
  const zones = cfg.widgets;
  const [catalog, setCatalog] = useState<WidgetCatalogItem[]>([]);
  const [adding, setAdding] = useState(false);
  const [addZone, setAddZone] = useState<ZoneName>("right");
  const [editing, setEditing] = useState<WidgetSlot | null>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const zoneEls = useRef<Partial<Record<ZoneName, HTMLDivElement | null>>>({});
  const focusKind = useRef<string | null>(null);

  useEffect(() => {
    api
      .taskbarWidgetCatalog()
      .then(setCatalog)
      .catch(() => setCatalog(WIDGET_KINDS.map((kind) => ({ kind, nameKey: `taskbar.widget.${kind}.name`, descKey: `taskbar.widget.${kind}.desc`, singleton: true }))));
  }, []);

  // Keep keyboard focus on the widget that was just moved (moving between zones re-creates the element).
  useEffect(() => {
    if (!focusKind.current) return;
    const el = document.querySelector<HTMLElement>(`[data-wkind="${focusKind.current}"]`);
    focusKind.current = null;
    el?.focus();
  });

  // Esc cancels the drag.
  useEffect(() => {
    if (!drag) return;
    const onKey = (e: globalThis.KeyboardEvent) => e.key === "Escape" && setDrag(null);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [drag]);

  const commit = (next: Zones) => set({ widgets: next });
  const present = new Set(ZONE_NAMES.flatMap((z) => zones[z].map(kindOf)));

  /** Drop slot under the pointer: the zone containing that point + the number of widgets above the point (by vertical midpoint). */
  const slotAt = (x: number, y: number): WidgetSlot | null => {
    for (const zone of ZONE_NAMES) {
      const el = zoneEls.current[zone];
      if (!el) continue;
      const r = el.getBoundingClientRect();
      if (x < r.left || x > r.right || y < r.top || y > r.bottom) continue;
      let index = 0;
      el.querySelectorAll<HTMLElement>("[data-chip]").forEach((c) => {
        const cr = c.getBoundingClientRect();
        if (cr.top + cr.height / 2 < y) index += 1;
      });
      return { zone, index };
    }
    return null;
  };

  const down = (e: ReactPointerEvent<HTMLDivElement>, zone: ZoneName, index: number, kind: string) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag({ from: { zone, index }, kind, startX: e.clientX, startY: e.clientY, x: e.clientX, y: e.clientY, moved: false, to: null });
  };
  const move = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag) return;
    const moved = drag.moved || Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) > DRAG_THRESHOLD;
    setDrag({ ...drag, x: e.clientX, y: e.clientY, moved, to: moved ? slotAt(e.clientX, e.clientY) : null });
  };
  const up = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag) return;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    const to = drag.moved ? slotAt(e.clientX, e.clientY) : null;
    const done = drag;
    setDrag(null);
    if (done.moved && to) {
      focusKind.current = done.kind;
      commit(moveWidget(zones, { from: done.from, to }));
    }
  };

  const key = (e: KeyboardEvent<HTMLDivElement>, zone: ZoneName, index: number, kind: string) => {
    const zi = ZONE_NAMES.indexOf(zone);
    let to: WidgetSlot | null = null;
    if (e.key === "ArrowUp" && index > 0) to = { zone, index: index - 1 };
    else if (e.key === "ArrowDown" && index < zones[zone].length - 1) to = { zone, index: index + 2 };
    else if (e.key === "ArrowLeft" && zi > 0) to = { zone: ZONE_NAMES[zi - 1], index: zones[ZONE_NAMES[zi - 1]].length };
    else if (e.key === "ArrowRight" && zi < 2) to = { zone: ZONE_NAMES[zi + 1], index: zones[ZONE_NAMES[zi + 1]].length };
    if (to) {
      e.preventDefault();
      focusKind.current = kind;
      commit(moveWidget(zones, { from: { zone, index }, to }));
    } else if (e.key === "Delete") {
      e.preventDefault();
      commit(removeWidget(zones, zone, index));
    } else if (e.key === "Enter") {
      e.preventDefault();
      setEditing({ zone, index });
    }
  };

  const target = drag?.moved ? drag.to : null;
  const editItem = editing ? zones[editing.zone][editing.index] : undefined;
  return (
    <Group title={t("taskbar.group.widgets")} en="WIDGETS">
      <div className={`tb-zones${drag?.moved ? " dragging" : ""}`}>
        {ZONE_NAMES.map((zone) => {
          const list = zones[zone];
          return (
            <div
              key={zone}
              ref={(el) => {
                zoneEls.current[zone] = el;
              }}
              className={`tb-zone${target?.zone === zone ? " drop" : ""}`}
              role="list"
              aria-label={t(`taskbar.zone.${zone}`)}
            >
              <b>{t(`taskbar.zone.${zone}`)}</b>
              {list.length === 0 && <span className="tb-empty">{t("taskbar.widgets.empty")}</span>}
              {list.map((w, i) => {
                const kind = kindOf(w);
                const gap = target?.zone === zone ? (target.index === i ? " gap-before" : target.index === list.length && i === list.length - 1 ? " gap-after" : "") : "";
                const dragged = drag?.moved && drag.from.zone === zone && drag.from.index === i;
                return (
                  <div
                    key={kind}
                    data-chip
                    data-wkind={kind}
                    role="listitem"
                    tabIndex={0}
                    className={`tb-chip${gap}${dragged ? " ghosted" : ""}`}
                    onPointerDown={(e) => down(e, zone, i, kind)}
                    onPointerMove={move}
                    onPointerUp={up}
                    onPointerCancel={() => setDrag(null)}
                    onDoubleClick={() => setEditing({ zone, index: i })}
                    onKeyDown={(e) => key(e, zone, i, kind)}
                  >
                    <span className="tb-name ellipsis">{t(`taskbar.widget.${kind}.name`)}</span>
                    <button className="tb-x" aria-label={t("taskbar.widgets.remove")} title={t("taskbar.widgets.remove")} onClick={() => commit(removeWidget(zones, zone, i))}>
                      ×
                    </button>
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>
      {drag?.moved && (
        <div className="tb-ghost" style={{ left: drag.x + 10, top: drag.y + 10 }}>
          {t(`taskbar.widget.${drag.kind}.name`)}
        </div>
      )}
      <Row>
        <IconButton label={t("taskbar.widgets.add")} onClick={() => setAdding((a) => !a)} />
        {adding && (
          <Segmented<ZoneName>
            options={ZONE_NAMES.map((z) => ({ value: z, label: t(`taskbar.zone.${z}`) }))}
            value={addZone}
            onChange={setAddZone}
          />
        )}
      </Row>
      {adding && (
        <div className="tb-cat" role="list" aria-label={t("taskbar.widgets.add_to")}>
          {catalog.map((c) => {
            const has = present.has(c.kind);
            return (
              <button
                key={c.kind}
                role="listitem"
                disabled={has}
                title={has ? t("taskbar.widgets.added") : t(c.descKey)}
                onClick={() => {
                  commit(addWidget(zones, addZone, c.kind));
                  setAdding(false);
                }}
              >
                <b>{t(c.nameKey)}</b>
                <small>{has ? t("taskbar.widgets.added") : t(c.descKey)}</small>
              </button>
            );
          })}
        </div>
      )}
      <p className="lb-hint">{t("taskbar.widgets.hint")}</p>
      {editing && editItem !== undefined && (
        <OptionsDialog
          kind={kindOf(editItem)}
          initial={optionsOf(editItem)}
          onCancel={() => setEditing(null)}
          onSave={(o) => {
            commit(setWidgetOptions(zones, editing.zone, editing.index, o));
            setEditing(null);
          }}
        />
      )}
    </Group>
  );
}
