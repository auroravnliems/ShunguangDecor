// Decides whether to ask for confirmation before toggling a module switch (only when turning OFF and the module has a registered prompt).
export function needsDisableGuard<G>(guards: Partial<Record<string, G>>, id: string, want: boolean): boolean {
  return !want && guards[id] !== undefined;
}
