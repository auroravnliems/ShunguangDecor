"use client";
// A module's config for its page: read when the page opens, re-read when Rust reports a change (profile load,
// file import), written after a short delay so dragging a slider does not call the command continuously.
import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "./api";
import { useT } from "./i18n";
import { useApp } from "./store";
import type { ModuleId } from "./types";

/** Delay before writing (ms). */
export const SAVE_DELAY = 150;

export interface ModuleConfig<T> {
  /** `null` until the read finishes. */
  cfg: T | null;
  /** Changes part of the config: the UI updates immediately, the core receives it after `SAVE_DELAY`. */
  update: (patch: Partial<T>) => void;
  /** Writes anything still pending right away (e.g. before enabling the module). */
  flush: () => Promise<void>;
}

/** `normalize` turns raw data (possibly missing keys/wrong types) into a complete config with default values. */
export function useModuleConfig<T extends object>(id: ModuleId, normalize: (raw: Record<string, unknown>) => T): ModuleConfig<T> {
  const { tt } = useT();
  const { toast } = useApp();
  const [cfg, setCfg] = useState<T | null>(null);
  const latest = useRef<T | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const norm = useRef(normalize);
  norm.current = normalize;

  const load = useCallback(() => {
    api
      .getModuleConfig(id)
      .then((raw) => {
        const c = norm.current(raw ?? {});
        latest.current = c;
        setCfg(c);
      })
      .catch((e) => toast("err", api.errorText(e, tt)));
  }, [id, toast, tt]);

  const write = useCallback(async () => {
    timer.current = null;
    if (!latest.current) return;
    try {
      await api.setModuleConfig(id, latest.current as Record<string, unknown>);
    } catch (e) {
      toast("err", api.errorText(e, tt));
    }
  }, [id, toast, tt]);

  useEffect(() => {
    load();
    const off = api.onModuleConfigChanged((changed) => {
      if (changed === id) load();
    });
    return () => {
      off.then((f) => f());
      // Leaving the page with unwritten changes: write them out.
      if (timer.current) {
        clearTimeout(timer.current);
        void write();
      }
    };
  }, [id, load, write]);

  const update = useCallback(
    (patch: Partial<T>) => {
      if (!latest.current) return;
      const next = { ...latest.current, ...patch };
      latest.current = next;
      setCfg(next);
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => void write(), SAVE_DELAY);
    },
    [write],
  );

  const flush = useCallback(async () => {
    if (timer.current) {
      clearTimeout(timer.current);
      await write();
    }
  }, [write]);

  return { cfg, update, flush };
}
