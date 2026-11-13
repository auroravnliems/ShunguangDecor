"use client";
// A page showing every control (dev build only, Ctrl+Shift+G) to preview the ones not yet used.
// The text here is sample data for developers and is not shown to users.
import { useState } from "react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Group, IconButton, Row, Skeleton } from "@/components/Group";
import { JoinButton } from "@/components/JoinButton";
import { PageHeader } from "@/components/PageHeader";
import { RangeSlider } from "@/components/RangeSlider";
import { Segmented } from "@/components/Segmented";
import { SelectBox } from "@/components/SelectBox";
import { SubTabs, TabPanel } from "@/components/SubTabs";
import { Swatches } from "@/components/Swatches";
import { Toggle } from "@/components/Toggle";
import { useApp } from "@/lib/store";

export function GalleryView() {
  const { toast } = useApp();
  const [tab, setTab] = useState<"a" | "b" | "c">("a");
  const [on, setOn] = useState(true);
  const [seg, setSeg] = useState("bot");
  const [h, setH] = useState(40);
  const [color, setColor] = useState("#141826");
  const [sel, setSel] = useState<string>("one");
  const [dlg, setDlg] = useState(false);
  return (
    <>
      <PageHeader title="Gallery" en="DEV · COMPONENTS" toggle={{ on, onChange: setOn }} right={<IconButton label="Toast" onClick={() => toast("ok", "Đã lưu bộ cấu hình")} />} />
      <div className="a" style={{ position: "absolute", left: 110, top: 84, ["--d" as string]: ".2s" }}>
        <SubTabs tabs={[{ id: "a", label: "Vị trí & cỡ" }, { id: "b", label: "Nền & màu" }, { id: "c", label: "Chữ" }]} value={tab} onChange={setTab} />
      </div>
      <div className="a" style={{ position: "absolute", left: 110, top: 128, width: 560, ["--d" as string]: ".3s" }}>
        <TabPanel tab={tab}>
          <Group title="Vị trí & kích thước" en="POSITION & SIZE">
            <Row label="Cạnh màn hình">
              <Segmented options={[{ value: "top", label: "Trên" }, { value: "bot", label: "Dưới" }]} value={seg} onChange={setSeg} />
            </Row>
            <Row label="Chiều cao">
              <RangeSlider min={28} max={64} value={h} onChange={setH} format={(v) => `${v} px`} />
            </Row>
            <Row label="Màu nền">
              <Swatches colors={["#141826", "#ffffff", "#3b7be9", "#e889c1", "#29c483"]} value={color} onChange={setColor} />
            </Row>
            <Row label="Làm mờ">
              <Toggle on={on} onChange={setOn} />
              <Toggle on={false} disabled onChange={() => {}} title="disabled" />
            </Row>
            <Row label="Ô chọn">
              <SelectBox options={[{ value: "one", label: "Mặc định của tôi" }, { value: "two", label: "Tối giản" }]} value={sel} onChange={setSel} />
            </Row>
          </Group>
        </TabPanel>
      </div>
      <div className="a" style={{ position: "absolute", left: 700, top: 128, display: "flex", flexDirection: "column", gap: 10, ["--d" as string]: ".4s" }}>
        <JoinButton label="Đổi hình nền" sub="CHANGE WALLPAPER" onClick={() => setDlg(true)} />
        <JoinButton label="Nhập con trỏ" sub="IMPORT CURSORS" variant="ghost" />
        <div style={{ display: "flex", gap: 10 }}>
          <JoinButton label="Nhập bộ con trỏ" size="small" />
          <JoinButton label="Về mặc định" size="small" variant="ghost" />
        </div>
        <div style={{ width: 420 }}>
          <Skeleton rows={2} />
        </div>
      </div>
      {dlg && (
        <ConfirmDialog title="Khôi phục mặc định Windows?" message="Hộp xác nhận mẫu." danger onConfirm={() => setDlg(false)} onCancel={() => setDlg(false)} />
      )}
    </>
  );
}
