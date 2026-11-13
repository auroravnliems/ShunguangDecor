import type { TaskbarConfig } from "@/lib/taskbar";

/** Props shared by the tabs: the current config + the change function (which already goes through the confirmation step when the config is hand-edited). */
export interface TabProps {
  cfg: TaskbarConfig;
  set: (patch: Partial<TaskbarConfig>) => void;
}
