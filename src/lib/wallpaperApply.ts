// Sets the wallpaper for one screen from outside the Wallpaper page (the thumbnail strip on the Home page).
import * as api from "./api";
import type { ModuleInfo } from "./types";
import { normalizeWallpaperConfig, withWallpaper } from "./wallpaper";

/** Writes the wallpaper into the layout of `monitor` (number), then turns the Wallpaper module on if it is off. Returns the module info when it was just turned on. */
export async function setWallpaperOn(monitor: number, id: string, enabled: boolean): Promise<ModuleInfo | null> {
  const cfg = normalizeWallpaperConfig((await api.getModuleConfig("wallpaper")) ?? {});
  await api.setModuleConfig("wallpaper", { ...cfg, layout: withWallpaper(cfg, monitor, id) });
  return enabled ? null : api.setModuleEnabled("wallpaper", true);
}
