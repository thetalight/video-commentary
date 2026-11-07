// Development-only browser regression fixture: no real task, disk mutation or AI.
import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { LegacyMigrationPanel } from "../src/components/LegacyMigrationPanel";

const counters = { status: 0, migration: 0 };
const mode = new URLSearchParams(location.search).get("mode") ?? "success";
Object.assign(window, { __migrationTest: counters, __TAURI_INTERNALS__: {
  invoke: async (command: string) => {
    if (command === "episode_migration_status") {
      counters.status++;
      if (mode === "status-error") throw new Error("synthetic status error");
      return { can_migrate: mode !== "stale", reason: mode === "stale" ? "选片材料已变化，请重新生成整集稿" : "合成材料一致，可以迁移" };
    }
    if (command === "migrate_episode") {
      counters.migration++;
      await new Promise(resolve => setTimeout(resolve, 300));
      if (mode === "failure") throw new Error("synthetic migration failure");
      return { schema_version: 2, revision: "synthetic-v2" };
    }
    throw new Error("Fixture blocked unexpected IPC");
  },
} });
function Fixture() {
  const [done, setDone] = useState(false);
  const [busy, setBusy] = useState(false);
  const [regenerate, setRegenerate] = useState(false);
  return <main>
    <p>仅合成任务，不访问真实数据</p><p>父级忙碌：{busy ? "是" : "否"}</p>
    {done ? <p role="status">迁移成功，下一步重新评审</p> : <LegacyMigrationPanel taskId="synthetic" revision="synthetic-v1" disabledReason="" onBusyChange={setBusy} onMigrated={() => setDone(true)} onRegenerate={() => setRegenerate(true)} />}
    {regenerate && <p role="status">已打开重新生成确认（尚未调用AI）</p>}
  </main>;
}
createRoot(document.getElementById("root")!).render(<Fixture />);

if (new URLSearchParams(location.search).has("auto")) {
  let phase = 0;
  const timer = window.setInterval(() => {
    const checkbox = document.querySelector<HTMLInputElement>('input[type="checkbox"]');
    const buttons = Array.from(document.querySelectorAll<HTMLButtonElement>("button"));
    if ((mode === "success" || mode === "failure") && phase === 0 && checkbox) {
      const button = buttons.find(b => b.textContent === "归档并迁移到v2");
      if (!button?.disabled) throw new Error("Migration must require explicit confirmation");
      checkbox.click(); phase = 1; return;
    }
    if (phase === 1) {
      const button = buttons.find(b => b.textContent === "归档并迁移到v2");
      if (button && !button.disabled) { button.click(); button.click(); phase = 2; }
      return;
    }
    const text = document.body.textContent ?? "";
    const passed = mode === "success" ? text.includes("迁移成功") && counters.migration === 1
      : mode === "failure" ? text.includes("迁移未完成") && text.includes("synthetic migration failure") && counters.migration === 1
      : mode === "stale" ? text.includes("选片材料已变化") && counters.migration === 0 && !checkbox
      : mode === "status-error" ? text.includes("无法检查迁移条件") && counters.migration === 0
      : false;
    if (passed) {
      clearInterval(timer);
      const result = document.createElement("pre"); result.id = "test-result";
      result.textContent = `PASS ${mode} status=${counters.status} migration=${counters.migration}`;
      document.body.appendChild(result);
    }
  }, 50);
}
