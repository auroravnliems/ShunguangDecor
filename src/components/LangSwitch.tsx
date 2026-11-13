"use client";
// A "VI/EN" button with a diagonal slash; clicking opens a list of 10 languages (native names). Arrow keys/Enter/Esc work.
import { useEffect, useRef, useState } from "react";
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";
import { LANGS, langInfo } from "@/lib/langs";
import { useApp } from "@/lib/store";

export function LangSwitch() {
  const { lang, t, tt } = useT();
  const { applySettings, toast } = useApp();
  const [open, setOpen] = useState(false);
  const [hi, setHi] = useState(0);
  const box = useRef<HTMLDivElement>(null);
  const current = langInfo(lang);
  const other = lang === "en" ? langInfo("vi") : langInfo("en");

  useEffect(() => {
    if (!open) return;
    setHi(Math.max(0, LANGS.findIndex((l) => l.code === lang)));
    const onDown = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open, lang]);

  const pick = (code: string) => {
    setOpen(false);
    if (code === lang) return;
    api
      .setLanguage(code)
      .then(applySettings)
      .catch((e) => toast("err", api.errorText(e, tt)));
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (!open) return;
    if (e.key === "Escape") setOpen(false);
    else if (e.key === "ArrowDown") setHi((h) => (h + 1) % LANGS.length);
    else if (e.key === "ArrowUp") setHi((h) => (h + LANGS.length - 1) % LANGS.length);
    else if (e.key === "Enter") pick(LANGS[hi].code);
    else return;
    e.preventDefault();
  };

  return (
    <div className="langw" ref={box} onKeyDown={onKey}>
      <button className="lang" title={t("header.language")} aria-haspopup="listbox" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <span className="a">{current.short}</span>
        <span className="b">{other.short}</span>
      </button>
      {open && (
        <div className="menu" role="listbox">
          {LANGS.map((l, i) => (
            <button
              key={l.code}
              role="option"
              aria-selected={l.code === lang}
              className={[l.code === lang ? "on" : "", i === hi ? "hi" : ""].join(" ").trim() || undefined}
              onMouseEnter={() => setHi(i)}
              onClick={() => pick(l.code)}
            >
              {l.native}
              <small>{l.short}</small>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
