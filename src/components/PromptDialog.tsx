"use client";
// Dialog asking for a single line of text (e.g. a config preset name), built on ConfirmDialog.
import { useEffect, useRef, useState } from "react";
import { ConfirmDialog } from "./ConfirmDialog";

export function PromptDialog(p: {
  title: string;
  label: string;
  initial: string;
  confirmLabel?: string;
  maxLength?: number;
  onConfirm: (value: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(p.initial);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);
  return (
    <ConfirmDialog
      title={p.title}
      confirmLabel={p.confirmLabel}
      message={
        <>
          {p.label}
          <input ref={input} value={value} maxLength={p.maxLength} onChange={(e) => setValue(e.target.value)} spellCheck={false} />
        </>
      }
      onConfirm={() => value.trim() && p.onConfirm(value)}
      onCancel={p.onCancel}
    />
  );
}
