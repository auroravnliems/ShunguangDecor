"use client";
// Properties tab: the LivelyProperties panel of the selected wallpaper.
import { useCallback, useEffect, useState } from "react";
import { Group, IconButton } from "@/components/Group";
import { LivelyControls } from "@/components/LivelyControls";
import * as api from "@/lib/api";
import { useT } from "@/lib/i18n";
import type { PropertiesView, WallpaperItem } from "@/lib/wallpaper";
import type { WallpaperCtx } from "./ctx";

export function PropsTab({ ctx, item }: { ctx: WallpaperCtx; item: WallpaperItem | undefined }) {
  const { t } = useT();
  const [view, setView] = useState<PropertiesView | null | undefined>(undefined);
  const id = item?.id;
  const { fail } = ctx;

  const load = useCallback(() => {
    if (!id) return;
    api
      .wallpaperProperties(id)
      .then(setView)
      .catch((e) => {
        setView(null);
        fail(e);
      });
  }, [id, fail]);
  useEffect(() => {
    setView(undefined);
    load();
  }, [load]);

  if (!item) return <div className="wp-empty">{t("wallpaper.props.pick")}</div>;
  if (view === null) return <div className="wp-empty">{t("wallpaper.props.none")}</div>;
  if (!view) return null;
  return (
    <div className="wp-props">
      <Group title={t("wallpaper.props.for", { name: item.title })} en="CUSTOMIZE">
        <LivelyControls
          controls={view.controls}
          values={view.values}
          onChange={(name, value) => void api.wallpaperSetProperty(item.id, name, value).catch(fail)}
          onPress={(name) => void api.wallpaperSetProperty(item.id, name, true).catch(fail)}
        />
        <div className="wp-reset">
          <IconButton
            label={t("wallpaper.props.reset")}
            onClick={() =>
              void api
                .wallpaperResetProperties(item.id)
                .then(load)
                .catch(fail)
            }
          />
        </div>
      </Group>
    </div>
  );
}
