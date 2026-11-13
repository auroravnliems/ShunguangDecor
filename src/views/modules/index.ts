// The page body of each part. A part without a real page yet gets a "coming soon" frame from ModuleView.
import type { ComponentType } from "react";
import type { ModuleId, ModuleInfo } from "@/lib/types";
import { CursorView } from "./CursorView";
import { DisableGuard, type DisableGuardProps } from "./features/DisableGuard";
import { FeaturesView } from "./FeaturesView";
import { LabelsView } from "./LabelsView";
import { TaskbarView } from "./TaskbarView";
import { WallpaperView } from "./WallpaperView";

export interface ModuleBodyProps {
  info: ModuleInfo;
}

export const MODULE_BODIES: Partial<Record<ModuleId, ComponentType<ModuleBodyProps>>> = {
  wallpaper: WallpaperView,
  cursor: CursorView,
  labels: LabelsView,
  taskbar: TaskbarView,
  features: FeaturesView,
};

/** Dialog asked before TURNING OFF a part's toggle (only for parts that need it): `onProceed` = allow turning off, `onCancel` = keep it on. */
export const MODULE_DISABLE_GUARDS: Partial<Record<ModuleId, ComponentType<DisableGuardProps>>> = {
  features: DisableGuard,
};
