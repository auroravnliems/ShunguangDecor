// Scales the 1280×760 design stage to fit the window, always filling it (no gray borders).
export const DESIGN_W = 1280;
export const DESIGN_H = 760;

export function fitStage(winW: number, winH: number): { zoom: number; width: number; height: number } {
  const w = Math.max(1, winW);
  const h = Math.max(1, winH);
  const zoom = Math.min(w / DESIGN_W, h / DESIGN_H);
  return { zoom, width: w / zoom, height: h / zoom };
}
