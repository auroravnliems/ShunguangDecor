"use client";
// Header bar: the window can be dragged by the bar background and the logo block (data-tauri-drag-region).
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import { LangSwitch } from "./LangSwitch";
import { NavMenu } from "./NavMenu";
import { WindowButtons } from "./WindowButtons";

export function Header() {
  const { t, tt } = useT();
  const { page, navigate, toast } = useApp();
  return (
    <header data-tauri-drag-region>
      <div className="logo" data-tauri-drag-region>
        <div className="ic" data-tauri-drag-region />
        <div className="wm" data-tauri-drag-region>
          ShunguangDecor<small data-tauri-drag-region>DECORATE · YOUR · WORLD</small>
        </div>
      </div>
      <NavMenu page={page} onGo={navigate} />
      <div className="hsep" />
      <LangSwitch />
      <button className={`gear${page === "settings" ? " on" : ""}`} title={t("header.settings")} onClick={() => navigate("settings")}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" />
        </svg>
      </button>
      <button className="discord" onClick={() => api.openLink("discord").catch((e) => toast("err", api.errorText(e, tt)))}>
        <svg viewBox="0 0 24 24" fill="#fff">
          <path d="M19.5 5.3A17 17 0 0 0 15.3 4l-.5 1a15.6 15.6 0 0 0-5.6 0l-.5-1a17 17 0 0 0-4.2 1.3C1.8 9.2 1.1 13 1.4 16.8a17 17 0 0 0 5.2 2.6l1.1-1.7a11 11 0 0 1-1.7-.8l.4-.3a12.2 12.2 0 0 0 11.2 0l.4.3-1.7.8 1.1 1.7a17 17 0 0 0 5.2-2.6c.4-4.4-.7-8.2-3.1-11.5ZM8.5 14.5c-1 0-1.9-1-1.9-2.1s.8-2.1 1.9-2.1 1.9 1 1.9 2.1-.8 2.1-1.9 2.1Zm7 0c-1 0-1.9-1-1.9-2.1s.8-2.1 1.9-2.1 1.9 1 1.9 2.1-.8 2.1-1.9 2.1Z" />
        </svg>
        <span>DISCORD</span>
      </button>
      <WindowButtons />
    </header>
  );
}
