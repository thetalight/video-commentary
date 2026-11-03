import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { convertFileSrc } from "@tauri-apps/api/core";
import { generateShorts, loadShorts, renderShort, revealPath, rewriteShort } from "../lib/api";
import type { ShortItem, ShortScript, ShortsBundle } from "../lib/types";

function clock(seconds: number) {
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

// A rough pre-render estimate; the final duration comes from local speech audio.
function spokenSeconds(plan: ShortScript) {
  return Math.round(plan.paragraphs.reduce((n, p) => n + Array.from(p.narration).length, 0) / 4.5);
}

function renderedCount(bundle: ShortsBundle | null) {
  return bundle?.items.filter(item => item.output_path).length ?? 0;
}

interface ShortsPanelProps {
  taskId: string;
  disabled?: boolean;
  onRenderedChange?: (rendered: number) => void;
}

export function ShortsPanel({ taskId, disabled, onRenderedChange }: ShortsPanelProps) {
  const onRenderedChangeRef = useRef(onRenderedChange);
  onRenderedChangeRef.current = onRenderedChange;
  const [bundle, setBundle] = useState<ShortsBundle | null>(null);
  const [edits, setEdits] = useState<Record<string, ShortScript>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    setBundle(null); setEdits({}); setError(""); setMessage("");
    loadShorts(taskId).then(value => {
      if (active) {
        setBundle(value);
        onRenderedChangeRef.current?.(renderedCount(value));
      }
    })
      .catch(e => { if (active) setError(String(e)); })
      .finally(() => { if (active) setLoading(false); });
    const subscription = listen<{ task_id: string; message: string }>("shorts-progress", event => {
      if (active && event.payload.task_id === taskId) setMessage(event.payload.message);
    });
    return () => { active = false; void subscription.then(unlisten => unlisten()); };
  }, [taskId]);

  function accept(next: ShortsBundle) {
    setBundle(next);
    onRenderedChange?.(renderedCount(next));
  }

  function dropEdit(id: string) {
    setEdits(current => { const { [id]: _, ...rest } = current; return rest; });
  }

  async function run<T>(label: string, action: () => Promise<T>) {
    setBusy(label); setError("");
    try { return await action(); }
    catch (e) { setError(String(e)); return undefined; }
    finally { setBusy(null); }
  }

  async function generateAll() {
    if (bundle && !window.confirm("将重新扫描全片并覆盖当前全部短片文案；已生成的视频文件保留在任务目录。是否继续？")) return;
    const next = await run("generate", () => generateShorts(taskId));
    if (next) { accept(next); setEdits({}); }
  }

  async function rewrite(item: ShortItem) {
    const next = await run(`${item.id}:rewrite`, () => rewriteShort(taskId, item.id));
    if (next) { accept(next); dropEdit(item.id); setMessage(`「${item.topic.title}」已重写`); }
  }

  async function renderOne(item: ShortItem) {
    const plan = edits[item.id] ?? item.plan;
    if (!plan) return false;
    const next = await run(`${item.id}:render`, () => renderShort(taskId, item.id, plan, item.revision));
    if (!next) return false;
    accept(next); dropEdit(item.id);
    setMessage(`「${item.topic.title}」视频已生成`);
    return true;
  }

  async function renderAll() {
    for (const item of bundle?.items ?? []) {
      if (!item.plan || (item.output_path && !edits[item.id])) continue;
      if (!(await renderOne(item))) return;
    }
    setMessage("全部短片视频已生成");
  }

  function updatePlan(item: ShortItem, plan: ShortScript) {
    setEdits(current => ({ ...current, [item.id]: plan }));
  }

  const locked = Boolean(busy) || loading || Boolean(disabled);
  const items = bundle?.items ?? [];
  const pending = items.filter(item => item.plan && (!item.output_path || edits[item.id])).length;

  return <section className="video-preview-panel shorts-panel" aria-label="短视频解说">
    <div className="eyebrow">SHORTS · 1 分钟竖屏解说</div>
    <h3>从全片挑出若干个独立看点</h3>
    <p>AI 扫描全片字幕，挑出名场面、反转、冲突等可独立观看的看点，条数按片长自动决定（约每 10 分钟一条，最多 8 条）。每条单独写约 1 分钟文案，经逐段评审后由你审稿，再生成 9:16 竖屏视频：模糊背景、居中原画、顶部标题和解说字幕；原声按设置压低铺底，可选背景音乐，出片前自动检查静音和黑场。</p>
    <div className="video-preview-tabs">
      <button type="button" className="primary" disabled={locked} onClick={() => void generateAll()}>
        {busy === "generate" ? "正在选题与写稿…" : bundle ? "全部重新选题" : "生成短视频选题与文案"}
      </button>
      <button type="button" disabled={locked || pending === 0} onClick={() => void renderAll()}>
        生成全部视频{pending > 0 ? `（${pending} 条待生成）` : ""}
      </button>
    </div>
    {message && <p role="status">{message}</p>}
    {error && <p role="alert" className="error-text">{error}</p>}

    {items.map((item, index) => {
      const plan = edits[item.id] ?? item.plan;
      const edited = Boolean(edits[item.id]);
      return <details key={item.id} className="segment-card short-card" open={index === 0}>
        <summary>
          <strong>{String(index + 1).padStart(2, "0")} · {plan?.title || item.topic.title}</strong>
          <span className="short-meta"> {item.topic.angle} · 看点 {item.topic.score}/5 · 原片 {clock(item.topic.start)}–{clock(item.topic.end)}
            {plan ? ` · 口播约 ${spokenSeconds(plan)} 秒` : ""}
            {item.error ? " · 写稿失败" : item.output_path && !edited ? ` · 已出片 ${item.duration_secs?.toFixed(1)} 秒` : ""}
          </span>
        </summary>
        <p className="short-premise">{item.topic.premise}</p>
        {item.topic.hook && <p className="short-premise">钩子：{item.topic.hook}</p>}
        {item.error && <p role="alert" className="error-text">{item.error}</p>}
        {item.warnings.map((warning, i) => <p key={i} role="status">{warning}</p>)}

        {plan && <>
          <label>竖屏标题
            <input type="text" disabled={locked} value={plan.title} onChange={e => updatePlan(item, { ...plan, title: e.target.value })} />
          </label>
          {plan.paragraphs.map((paragraph, pi) => <div key={pi} className="short-paragraph">
            <label>第 {pi + 1} 段 · {Array.from(paragraph.narration).length} 字 · {paragraph.shot_ids.length} 个镜头
              <textarea rows={3} disabled={locked} value={paragraph.narration} onChange={e => updatePlan(item, {
                ...plan, paragraphs: plan.paragraphs.map((p, i) => i === pi ? { ...p, narration: e.target.value } : p),
              })} />
            </label>
            <p className="field-help">字幕依据：{paragraph.evidence_quote}</p>
            <details><summary>调整本段画面（按时间顺序原速播放）</summary>
              {item.shots.map(shot => {
                const takenElsewhere = plan.paragraphs.some((p, i) => i !== pi && p.shot_ids.includes(shot.id));
                return <label key={shot.id} className="short-shot">
                  <input type="checkbox" disabled={locked || (takenElsewhere && !paragraph.shot_ids.includes(shot.id))}
                    checked={paragraph.shot_ids.includes(shot.id)} onChange={e => {
                      const ids = e.target.checked
                        ? [...paragraph.shot_ids, shot.id].sort((a, b) => a - b)
                        : paragraph.shot_ids.filter(id => id !== shot.id);
                      updatePlan(item, { ...plan, paragraphs: plan.paragraphs.map((p, i) => i === pi ? { ...p, shot_ids: ids } : p) });
                    }} /> {clock(shot.start)}–{clock(shot.end)} · {shot.text}
                </label>;
              })}
            </details>
          </div>)}
        </>}

        {edited && <p>当前修改尚未生成视频；下方如有视频，仍是上一版。</p>}
        <div className="button-row">
          <button type="button" disabled={locked} onClick={() => void rewrite(item)}>
            {busy === `${item.id}:rewrite` ? "重写中…" : "不满意，重写这条"}
          </button>
          <button type="button" className="primary" disabled={locked || !plan} onClick={() => void renderOne(item)}>
            {busy === `${item.id}:render` ? "生成中…" : item.output_path && !edited ? "重新生成视频" : "生成这条视频"}
          </button>
        </div>

        {item.output_path && <div className="short-video">
          <video key={item.output_path} src={convertFileSrc(item.output_path)} controls preload="metadata" playsInline />
          <button type="button" className="text-btn" onClick={() => void revealPath(item.output_path!)}>打开视频文件</button>
        </div>}
      </details>;
    })}
  </section>;
}
