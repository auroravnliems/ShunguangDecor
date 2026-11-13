"use client";
// The "Profiles" bar of a part: pick a profile, Save, Export file, Import file, Delete.
import { useCallback, useEffect, useState } from "react";
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import type { ModuleId, ProfileList } from "@/lib/types";
import { ConfirmDialog } from "./ConfirmDialog";
import { IconButton } from "./Group";
import { PromptDialog } from "./PromptDialog";
import { SelectBox } from "./SelectBox";

type Ask = { kind: "name" } | { kind: "overwrite"; name: string } | { kind: "delete"; name: string } | null;

export function ProfileBar({ module, disabled }: { module: ModuleId; disabled: boolean }) {
  const { t, tt } = useT();
  const { toast } = useApp();
  const [list, setList] = useState<ProfileList>({ profiles: [], active: null });
  const [ask, setAsk] = useState<Ask>(null);
  const fail = useCallback((e: unknown) => toast("err", api.errorText(e, tt)), [toast, tt]);

  const refresh = useCallback(() => {
    api.profileList(module).then(setList).catch(fail);
  }, [module, fail]);
  useEffect(refresh, [refresh]);

  const save = async (name: string, overwrite: boolean) => {
    setAsk(null);
    try {
      await api.profileSave(module, name, overwrite);
      toast("ok", t("toast.profile_saved", { name: name.trim() }));
      refresh();
    } catch (e) {
      if (e && typeof e === "object" && (e as { key?: string }).key === "err.profile.exists") setAsk({ kind: "overwrite", name });
      else fail(e);
    }
  };
  const load = (name: string) =>
    api
      .profileLoad(module, name)
      .then(() => {
        toast("ok", t("toast.profile_loaded", { name }));
        refresh();
      })
      .catch(fail);
  const exportFile = async () => {
    if (!list.active) return;
    const dest = await api.pickSavePath(`${list.active}.sgdecor`, "sgdecor", "ShunguangDecor");
    if (!dest) return;
    api
      .profileExport(module, list.active, dest)
      .then(() => toast("ok", t("toast.exported")))
      .catch(fail);
  };
  const importFile = async () => {
    const src = await api.pickOpenPath("sgdecor", "ShunguangDecor");
    if (!src) return;
    api
      .profileImport(module, src)
      .then((name) => {
        toast("ok", t("toast.profile_imported", { name }));
        refresh();
      })
      .catch(fail);
  };
  const remove = (name: string) => {
    setAsk(null);
    api
      .profileDelete(module, name)
      .then(() => {
        toast("ok", t("toast.profile_deleted", { name }));
        refresh();
      })
      .catch(fail);
  };

  return (
    <>
      {t("profile.label")}
      <SelectBox
        options={list.profiles.map((p) => ({ value: p.name, label: p.name }))}
        value={list.active}
        placeholder={list.profiles.length ? t("profile.choose") : t("profile.none")}
        disabled={disabled || list.profiles.length === 0}
        label={t("profile.label")}
        onChange={load}
      />
      <IconButton label={t("profile.save")} disabled={disabled} onClick={() => setAsk({ kind: "name" })} />
      <IconButton label={t("profile.export")} disabled={disabled || !list.active} title={list.active ? undefined : t("profile.export_hint")} onClick={exportFile} />
      <IconButton label={t("profile.import")} disabled={disabled} onClick={importFile} />
      <IconButton label={t("profile.delete")} disabled={disabled || !list.active} onClick={() => list.active && setAsk({ kind: "delete", name: list.active })} />

      {ask?.kind === "name" && (
        <PromptDialog
          title={t("profile.save_title")}
          label={t("profile.save_label")}
          initial={list.active ?? ""}
          confirmLabel={t("profile.save")}
          maxLength={64}
          onConfirm={(name) => save(name, false)}
          onCancel={() => setAsk(null)}
        />
      )}
      {ask?.kind === "overwrite" && (
        <ConfirmDialog
          title={t("profile.overwrite_title")}
          message={t("profile.overwrite_body", { name: ask.name.trim() })}
          confirmLabel={t("profile.overwrite_ok")}
          onConfirm={() => save(ask.name, true)}
          onCancel={() => setAsk(null)}
        />
      )}
      {ask?.kind === "delete" && (
        <ConfirmDialog
          title={t("profile.delete_title")}
          message={t("profile.delete_body", { name: ask.name })}
          confirmLabel={t("profile.delete")}
          danger
          onConfirm={() => remove(ask.name)}
          onCancel={() => setAsk(null)}
        />
      )}
    </>
  );
}
