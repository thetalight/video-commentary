import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { convertFileSrc } from "@tauri-apps/api/core";
import { generateEpisode, loadEpisode, renderEpisode, reviewEpisode, saveEpisode, revealPath } from "../lib/api";
import type { Episode, ShortScript } from "../lib/types";

export function EpisodePanel({ taskId, disabled, onRenderedChange }: { taskId: string; disabled?: boolean; onRenderedChange?: (count: number) => void }) {
  const callback = useRef(onRenderedChange);
  callback.current = onRenderedChange;
  const [episode, setEpisode] = useState<Episode | null>(null);
  const [draft, setDraft] = useState<ShortScript | null>(null);
  const [busy, setBusy] = useState("");
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [openShots, setOpenShots] = useState<Record<number, boolean>>({});
  useEffect(() => {
    let active = true;
    setLoading(true); setLoadFailed(false); setEpisode(null); setDraft(null); setError(""); setConfirmed(false);
    loadEpisode(taskId).then(value => { if (active) { setEpisode(value); setDraft(value?.plan ?? null); } })
      .catch(e => { if (active) { setError(String(e)); setLoadFailed(true); } })
      .finally(() => { if (active) setLoading(false); });
    const subscription = listen<{ task_id: string; message: string }>("episode-progress", e => {
      if (active && e.payload.task_id === taskId) setMessage(e.payload.message);
    });
    return () => { active = false; void subscription.then(unlisten => unlisten()); };
  }, [taskId]);
  useEffect(() => { if (!loading && !loadFailed) callback.current?.(episode?.output_path ? 1 : 0); }, [episode?.output_path, loading, loadFailed]);
  const dirty = Boolean(draft && episode && JSON.stringify(draft) !== JSON.stringify(episode.plan));
  const locked = Boolean(disabled || busy || loading || loadFailed);
  const reviewed = Boolean(episode && episode.reviewed_revision === episode.revision && !dirty);
  const renderBlocker = loading ? "正在读取整集稿件。"
    : loadFailed ? "稿件读取失败，请重新选择任务。"
    : disabled ? "素材准备尚在运行，请等待完成。"
    : busy ? `正在${busy}，请等待完成。`
    : !episode ? "请先生成整集解说全文。"
    : dirty ? "有未保存修改：请先保存草稿，再进行事实与全文连贯性评审。"
    : !reviewed ? episode.review_error
      ? `当前版本评审未通过：${episode.review_error}。可点“AI审查并自动修稿”处理当前稿；两轮后仍有问题需人工核对。不必重新下载或转写。`
      : "当前版本尚未通过评审，请点击“事实与全文连贯性评审”。"
    : !confirmed ? "评审已通过，请通读全文并勾选下方人工审稿确认。"
    : "";
  const chars = draft?.paragraphs.reduce((sum,p) => sum + Array.from(p.narration).length, 0) ?? 0;
  async function run(label: string, action: () => Promise<Episode>) {
    setBusy(label); setError(""); setConfirmed(false);
    try { const next = await action(); setEpisode(next); setDraft(next.plan); setMessage(`${label}已完成，请核对当前版本与警告`); }
    catch (e) { setError(String(e)); }
    finally { setBusy(""); }
  }
  function edit(plan: ShortScript) { setDraft(plan); setConfirmed(false); }
  return <section className="video-preview-panel" aria-label="整集长视频解说">
    <div className="eyebrow">EPISODE · 30% · 16:9</div>
    <h3>整集统一叙事，一篇全文，一条横屏成片</h3>
    <p>全集梳理后制定统一方案，优先一次写完整稿；超出单次输出预算时连续续写，不独立分章节。原片约30%为初始时长预算，事实与剧情完整性优先。原语言字幕和文案会发送到配置的 DeepSeek API，直接生成中文解说，不先全文翻译；证据引文保留原语言，不上传原片媒体。</p>
    <div className="button-row">
      <button className="primary" type="button" disabled={locked} onClick={() => {
        if (dirty && !window.confirm("有未保存修改，重新生成会丢弃这些编辑。继续？")) return;
        if (episode && !window.confirm("重新梳理整集并生成新全文会替换当前稿件；旧稿归档、旧短片数据和媒体文件保留。继续？")) return;
        void run("生成全文", () => generateEpisode(taskId, episode?.revision ?? null));
      }}>{busy === "生成全文" ? "全集梳理与全文写稿中…" : episode ? "重新生成整集稿" : "生成整集解说全文"}</button>
      <button type="button" disabled={locked || !dirty || !draft || !episode} onClick={() => episode && draft && void run("保存草稿", () => saveEpisode(taskId, draft, episode.revision))}>保存草稿</button>
      <button type="button" disabled={locked || dirty || !episode} onClick={() => episode && void run("AI审查与自动修稿", () => reviewEpisode(taskId, episode.revision))}>AI审查并自动修稿</button>
    </div>
    {message && <p role="status">{message}</p>}
    {error && <p className="error-text" role="alert">{error}</p>}
    {loadFailed && <p>读取失败时禁止覆盖生成；请检查任务记录后重新选择任务。</p>}
    {episode && draft && <>
      <p>{chars} 字 · 估计口播约 {(chars / 4 / 60).toFixed(1)} 分钟 · 目标 {(episode.target_secs / 60).toFixed(1)} 分钟（实测为准）</p>
      <p role="status">{dirty ? "有未保存修改，旧评审不再代表此稿" : reviewed ? "当前版本已通过程序、逐段事实和全文连贯性检查；仍需人工通读" : "当前稿尚未通过完整评审，不可出片"}</p>
      {episode.review_error && <p className="error-text" role="alert">{episode.review_error}</p>}
      {episode.warnings.map((warning,i) => <p key={i}>{warning}</p>)}
      <p className="field-help">AI自动检查事实与全文连贯性，最多两轮修改问题段。配置Tavily时可按需联网查词义/人名/背景；网络资料不替代原字幕。只有当前稿通过检查才可人工确认出片。</p>
      {(episode.research?.length ?? 0) > 0 && <details><summary>本轮联网查证来源（外部参考，不是原片证据）</summary>
        {episode.research!.map((record,i) => <div key={i}>
          <p>查询：{record.query.query} · 用途：{record.query.reason}</p>
          {record.error && <p role="status">{record.error}</p>}
          {record.sources.map((source,j) => <p key={j}>{source.title} · {source.url}</p>)}
        </div>)}
      </details>}
      <details><summary>全集叙事方案、人物与揭示次序</summary>
        <p>{episode.story.spine}</p>
        <ul>{episode.story.characters.map((person,i) => <li key={i}>{person}</li>)}</ul>
        <p>{episode.story.reveal_order}</p>
        <ol>{episode.story.beats.map((beat,i) => <li key={i}>{beat.task}</li>)}</ol>
      </details>
      <label>整集标题<input disabled={locked} value={draft.title} onChange={e => edit({ ...draft, title: e.target.value })} /></label>
      {draft.paragraphs.map((p,i) => <div className="short-paragraph" key={i}>
        <label>第 {i+1} 段 · {Array.from(p.narration).length} 字
          <textarea rows={4} disabled={locked} value={p.narration} onChange={e => edit({ ...draft, paragraphs: draft.paragraphs.map((value,j) => i === j ? { ...value, narration: e.target.value } : value) })} />
        </label>
        <label>连续字幕引文<input disabled={locked} value={p.evidence_quote} onChange={e => edit({ ...draft, paragraphs: draft.paragraphs.map((value,j) => i === j ? { ...value, evidence_quote: e.target.value } : value) })} /></label>
        <details onToggle={e => { const open = e.currentTarget.open; setOpenShots(current => ({ ...current, [i]: open })); }}><summary>选片与证据（稳定全片 ID；原速）</summary>
          {openShots[i] && episode.shots.map(shot => <label key={shot.id} className="short-shot">
            <input type="checkbox" disabled={locked || (!p.shot_ids.includes(shot.id) && draft.paragraphs.some((value,j) => j !== i && value.shot_ids.includes(shot.id)))} checked={p.shot_ids.includes(shot.id)} onChange={e => edit({ ...draft, paragraphs: draft.paragraphs.map((value,j) => i === j ? { ...value, shot_ids: e.target.checked ? [...value.shot_ids,shot.id].sort((a,b)=>a-b) : value.shot_ids.filter(id=>id!==shot.id) } : value) })} />
            #{shot.id} · {Math.floor(shot.start/60)}:{Math.floor(shot.start%60).toString().padStart(2,"0")} · {shot.text}
          </label>)}
        </details>
      </div>)}
      <label><input type="checkbox" disabled={locked || !reviewed} checked={confirmed} onChange={e => setConfirmed(e.target.checked)} />我已通读当前全文并核对警告，确认按16:9横屏出片（之后仍需完整看片验收）</label>
      {renderBlocker && <p id="episode-render-blocker" role="status" className="field-help">暂不能出片：{renderBlocker}</p>}
      <button type="button" className="primary" title={renderBlocker || "当前稿已满足出片条件"} aria-describedby={renderBlocker ? "episode-render-blocker" : undefined} disabled={locked || !reviewed || !confirmed} onClick={() => void run("横屏出片", () => renderEpisode(taskId, episode.revision, true))}>{busy === "横屏出片" ? "配音与出片中…" : "生成整集横屏视频"}</button>
      {episode.output_path && <div className="episode-video">
        <p>实测 {(episode.duration_secs! / 60).toFixed(1)} 分钟 · 待人工最终验收</p>
        <video style={{ width: "100%", aspectRatio: "16/9" }} key={episode.output_path} src={convertFileSrc(episode.output_path)} controls preload="metadata" />
        <button type="button" onClick={() => void revealPath(episode.output_path!)}>打开当前视频文件</button>
      </div>}
    </>}
  </section>;
}
