"use client";
// Sub-tab row: solid blue cells; when the tab content changes it is wrapped in `.shift` (slides in from the left) — see TabPanel.
import { Fragment, type ReactNode } from "react";

export function SubTabs<T extends string>(p: { tabs: { id: T; label: string }[]; value: T; onChange: (v: T) => void }) {
  return (
    <div className="subtabs" role="tablist">
      {p.tabs.map((tab, i) => (
        <Fragment key={tab.id}>
          {i > 0 && <i />}
          <button role="tab" aria-selected={tab.id === p.value} className={tab.id === p.value ? "on" : undefined} onClick={() => p.onChange(tab.id)}>
            {tab.label}
          </button>
        </Fragment>
      ))}
    </div>
  );
}

/** Content of the selected tab; `key` follows the tab so the animation replays on every change. */
export function TabPanel({ tab, children }: { tab: string; children: ReactNode }) {
  return (
    <div key={tab} className="shift" role="tabpanel">
      {children}
    </div>
  );
}
