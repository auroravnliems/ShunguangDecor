// What every tab of the Wallpaper page shares.
import type { MonitorView, ModuleInfo } from "@/lib/types";
import type { EngineState, WallpaperConfig, WallpaperItem } from "@/lib/wallpaper";

export interface WallpaperCtx {
  info: ModuleInfo;
  cfg: WallpaperConfig;
  update: (patch: Partial<WallpaperConfig>) => void;
  state: EngineState;
  items: WallpaperItem[];
  monitors: MonitorView[];
  /** The monitor being edited (number). */
  monitor: number;
  setMonitor: (n: number) => void;
  fail: (e: unknown) => void;
}
