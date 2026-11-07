// Synthetic browser fixture. No real tasks, files, providers or model calls.
import React from "react";
import { createRoot } from "react-dom/client";
import { EpisodePanel } from "../src/components/EpisodePanel";
import "../src/App.css";
const mode = new URLSearchParams(location.search).get("mode") ?? "success";
const counters = { review: 0, save: 0 };
const callbacks = new Map<number, (event: unknown) => void>();
let nextCallback = 1;
const paragraphs = ["小镇的钟突然停了。修钟的人却说，真正消失的不是时间，而是昨天寄来的那封信。", "他在抽屉里找到一把钥匙。门外的脚步声越来越近，这一次，他决定先把真相说出来。", "当旧门终于打开，失踪的人没有回来。留下的信解释了当年的选择，也让两人重新理解了彼此。"];
let episode = {
  schema_version: mode === "legacy" ? 1 : 2, revision: "synthetic-1", transcript_revision: "synthetic-t", source_revision: "synthetic-s",
  plan: { title: "小镇最后一封信", paragraphs: paragraphs.map((text, i) => ({ narration: text, evidence_quote: `Synthetic source line ${i}`, evidence_refs: [i], visual_selections: [{ candidate_id: i, start: i * 30, end: i * 30 + 20, must_keep_ranges: [] }] })) },
  shots: paragraphs.map((_, i) => ({ id: i, start: i * 30, end: i * 30 + 20, text: `Synthetic source line ${i}` })),
  story: { spine: "一封迟到的信，让三个被误解的选择重新连成完整因果。", characters: ["修钟人：始终保留着那封未寄出的信", "来访者：追查旧事，逐步理解当年的选择"], reveal_order: "先建立失踪之谜，再揭示钥匙的用途，结尾回收信件的意义。", beats: paragraphs.map((_, i) => ({ task: `节点 ${i + 1}：以新信息推进人物的选择`, shot_ids: [i] })), covered_events: [] },
  target_secs: 720, reviewed_revision: mode === "passed" ? "synthetic-1" : null, review_error: null, warnings: [], output_path: null, duration_secs: null,
};
Object.assign(window, { __episodeTest: counters, __TAURI_INTERNALS__: {
  transformCallback: (handler: (event: unknown) => void) => { const id = nextCallback++; callbacks.set(id, handler); return id; },
  convertFileSrc: (path: string) => path,
  invoke: async (command: string, args: Record<string, unknown>) => {
    if (command === "load_episode") return structuredClone(episode);
    if (command === "plugin:event|listen") return args.handler;
    if (command === "plugin:event|unlisten") return null;
    if (command === "episode_migration_status") return { can_migrate: true, reason: "合成素材一致" };
    if (command === "save_episode") { counters.save++; episode = { ...episode, plan: structuredClone(args.plan) as typeof episode.plan, revision: "synthetic-saved", reviewed_revision: null }; return structuredClone(episode); }
    if (command === "review_episode") {
      counters.review++;
      if (mode === "failure") throw new Error("合成后端：任务占用，请等待现有操作结束");
      callbacks.forEach(handler => handler({ event: "episode-progress", payload: { task_id: "synthetic", message: "已收到审稿请求，正在核对全文…" } }));
      await new Promise(resolve => setTimeout(resolve, 250));
      if (mode !== "unresolved") episode = { ...episode, reviewed_revision: episode.revision };
      return structuredClone(episode);
    }
    throw new Error(`Fixture blocked unexpected IPC: ${command}`);
  },
} });
createRoot(document.getElementById("root")!).render(<div className="app"><header className="app-header"><div className="brand"><div className="brand-icon">解</div><div><div className="eyebrow">STORYCUT / 解说工作台</div><h1>整集叙事工作室</h1><p className="tagline">一集故事 · 一篇全文 · 一条成片</p></div></div></header><main className="app-layout"><aside className="work-panel queue-panel"><div className="panel-content"><div className="eyebrow">素材与任务</div><h2>创建解说任务</h2><p>合成任务，不访问真实数据</p><div className="create-card"><label>视频链接<input type="url" placeholder="粘贴片源链接" /></label><button type="button" className="primary">下载并准备素材</button></div><p>01 · 小镇最后一封信</p></div></aside><div className="work-panel review-column"><div className="panel-content"><EpisodePanel taskId="synthetic" /></div></div></main></div>);

if (new URLSearchParams(location.search).has("auto")) {
  let phase = 0;
  const timer = window.setInterval(() => {
    const button = Array.from(document.querySelectorAll<HTMLButtonElement>("button")).find(b => b.textContent?.includes("开始AI审稿") || b.textContent?.includes("审稿进行中"));
    if (phase === 0 && button) {
      if (mode === "layout") {
        if (document.documentElement.scrollWidth > window.innerWidth + 1) throw new Error("Horizontal overflow at fixture width");
        const picture = Array.from(document.querySelectorAll<HTMLButtonElement>("button")).find(b => b.textContent?.startsWith("证据选片"))!;
        picture.click(); phase = 3; return;
      }
      if (mode === "legacy") {
        if (!button.disabled) throw new Error("Legacy review must be blocked");
        finish(); return;
      }
      if (mode === "dirty") {
        const textarea = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="第1段文稿"]')!;
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(textarea, "这里是用户修改后的合成稿件。");
        textarea.dispatchEvent(new Event("input", { bubbles: true })); phase = 1; return;
      }
      button.click(); button.click(); phase = 2; return;
    }
    if (phase === 3 && document.querySelector('select[aria-label="选择编辑段落"]')) {
      const delivery = Array.from(document.querySelectorAll<HTMLButtonElement>("button")).find(b => b.textContent?.startsWith("试听出片"))!;
      delivery.click(); phase = 4; return;
    }
    if (phase === 4 && Array.from(document.querySelectorAll<HTMLButtonElement>("button")).some(b => b.textContent === "生成整集横屏视频")) {
      if (document.documentElement.scrollWidth > window.innerWidth + 1) throw new Error("Delivery horizontal overflow");
      finish(); return;
    }
    if (phase === 1) {
      const saveReview = Array.from(document.querySelectorAll<HTMLButtonElement>("button")).find(b => b.textContent === "保存并开始AI审稿");
      if (saveReview) { saveReview.click(); phase = 2; } return;
    }
    if (phase === 2) {
      const body = document.body.textContent ?? "";
      if (mode === "failure" ? body.includes("合成后端：任务占用") : mode === "unresolved" ? body.includes("仍有待核问题") : body.includes("审稿完成 · 当前版本已通过")) {
        if (counters.review !== 1 || (mode === "dirty" && counters.save !== 1)) throw new Error("Duplicate or missing action");
        finish();
      }
    }
    function finish() { clearInterval(timer); const result = document.createElement("pre"); result.id = "test-result"; result.textContent = `PASS ${mode} save=${counters.save} review=${counters.review}`; document.body.append(result); }
  }, 50);
}
