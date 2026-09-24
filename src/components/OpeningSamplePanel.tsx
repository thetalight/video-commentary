import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { convertFileSrc } from "@tauri-apps/api/core";
import { generateOpeningSample, loadOpeningSample, renderOpeningSample, revealPath } from "../lib/api";
import type { OpeningSample } from "../lib/types";
import { readSampleReviews, type SampleReviews } from "../lib/api";

export function OpeningSamplePanel({ taskId, disabled }: { taskId: string; disabled?: boolean }) {
  const [sample, setSample] = useState<OpeningSample | null>(null);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [edited, setEdited] = useState(false);
  const [reviews, setReviews] = useState<SampleReviews>({ reviews: [] });

  useEffect(() => {
    let active = true;
    readSampleReviews(taskId).then(value => { if (active) setReviews(value); }).catch(() => {});
    loadOpeningSample(taskId).then(value => { if (active) setSample(value); })
      .catch(e => { if (active) setError(String(e)); })
      .finally(() => { if (active) setLoading(false); });
    const subscription = listen<{ task_id: string; message: string }>("sample-progress", event => {
      if (active && event.payload.task_id === taskId) setMessage(event.payload.message);
    });
    return () => { active = false; void subscription.then(unlisten => unlisten()); };
  }, [taskId]);

  async function run(render: boolean) {
    setBusy(true); setError(""); setMessage(render ? "正在准备样片配音…" : "正在准备字幕证据…");
    try {
      const result = render && sample
        ? await renderOpeningSample(taskId, sample.plan, sample.revision)
        : await generateOpeningSample(taskId);
      setSample(result); setEdited(false);
      setMessage(render ? "样片已生成，请播放检查文案、声音和画面是否一致。" : "样片稿已生成，请先审稿，再生成视频。AI 复核不代替人工确认。");
    } catch (e) { setError(String(e)); setMessage(""); }
    finally {
      setBusy(false);
      readSampleReviews(taskId).then(setReviews).catch(() => {});
    }
  }

  const locked = busy || loading || disabled;
  return <section className="video-preview-panel" aria-label="开头样片实验">
    <div className="eyebrow">OPENING LAB · 先看效果</div>
    <h3>先做 60–90 秒开头样片</h3>
    <p>独立重写开头：字幕证据 → 故事段落 → 多镜头原速剪辑。整片稿和已有成片不会被覆盖。</p>
    <p>Rig 接入模型 · 逐段评审 → 最多两轮局部修复。缺少证据或修改退步时停止，保留草稿与问题记录。</p>
    <p>取开头约 20 分钟内的有效字幕。本版仅配解说，不混原片对白、不加背景音乐。60–90 秒是建议，不是凑字门槛；画面不足会提示补选，不定格。</p>
    <div className="video-preview-tabs">
      <button type="button" disabled={locked} onClick={() => void run(false)}>{busy ? "样片处理中…" : sample ? "不满意，重写样片稿" : "生成开头样片稿"}</button>
      <button type="button" disabled={locked || !sample} onClick={() => void run(true)}>按此稿生成样片</button>
    </div>
    {message && <p role="status">{message}</p>}
    {error && <p role="alert" className="error-text">{error}</p>}
    {reviews.reviews.length > 0 && <details>
      <summary>查看最近一次创作的评审记录（{reviews.reviews.length} 次）</summary>
      {reviews.reviews.map((review,round) => <section key={round}>
        <h4>第 {round+1} 次评审</h4>
        {review.report.findings.map((finding,index) => <p key={index}>
          第 {finding.paragraph_index+1} 段 · {finding.verdict === "pass" ? "模型认为通过" : finding.verdict === "revise" ? "需要修改" : "证据不足"}：{finding.reason}
        </p>)}
      </section>)}
      <p>这是模型对相应版本的意见；程序校验或人工确认仍可能未通过。</p>
      {reviews.path && <button type="button" onClick={() => void revealPath(reviews.path!)}>打开草稿与完整评审记录</button>}
    </details>}
    {sample && <>
      {sample.warnings.map((warning,index) => <p key={index} role="status">{warning}</p>)}
      <p>预计口播 {Math.round(sample.plan.paragraphs.reduce((n,p) => n + Array.from(p.narration).length, 0) / 4)} 秒 · {sample.plan.paragraphs.length} 个意群。手改文案后请自行核实事实。</p>
      {sample.plan.paragraphs.map((paragraph, index) => <details key={index} open={index === 0} className="segment-card">
        <summary>第 {index+1} 段 · {paragraph.shot_ids.length} 个镜头</summary>
        <label>口语解说
          <textarea rows={4} disabled={locked} value={paragraph.narration} onChange={e => {
            setEdited(true);
            setSample({ ...sample, plan: { ...sample.plan, paragraphs: sample.plan.paragraphs.map((p,i) => i === index ? {...p,narration:e.target.value} : p) } });
          }} />
        </label>
        <p>字幕原文依据：{paragraph.evidence_quote}</p>
        <details><summary>查看 / 调整同情节画面（按时间顺序播放）</summary>
          {sample.shots.map(shot => <label key={shot.id} style={{display:"block",margin:"8px 0"}}>
            <input type="checkbox" disabled={locked || (!paragraph.shot_ids.includes(shot.id) && sample.plan.paragraphs.some((p,i) => i !== index && p.shot_ids.includes(shot.id)))} checked={paragraph.shot_ids.includes(shot.id)} onChange={e => {
              const ids = e.target.checked ? [...paragraph.shot_ids,shot.id].sort((a,b) => a-b) : paragraph.shot_ids.filter(id => id !== shot.id);
              setEdited(true);
              setSample({...sample,plan:{...sample.plan,paragraphs:sample.plan.paragraphs.map((p,i) => i === index ? {...p,shot_ids:ids} : p)}});
            }} /> {shot.start.toFixed(1)}–{shot.end.toFixed(1)}s · {shot.text}
          </label>)}
        </details>
      </details>)}
      {edited && <p>当前修改尚未生成视频；下方如有视频，仍是上一版。点击“按此稿生成样片”保存本次稿并试制。</p>}
      {sample.output_path && <>
        <h4>样片预览 · {sample.duration_secs?.toFixed(1)} 秒</h4>
        <video className="preview-video" key={sample.output_path} src={convertFileSrc(sample.output_path)} controls preload="metadata" playsInline />
        <button type="button" className="text-btn" onClick={() => void revealPath(sample.output_path!)}>打开样片文件</button>
      </>}
    </>}
  </section>;
}
