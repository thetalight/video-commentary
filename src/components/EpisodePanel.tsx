import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { convertFileSrc } from "@tauri-apps/api/core";
import { generateEpisode, loadEpisode, renderEpisode, reviewEpisode, saveEpisode, importReviewedEpisode, detectEpisodeScenes, previewEpisodeNarration, saveEpisodeStory, revealInFinder } from "../lib/api";
import type { Episode, EpisodeScript, EpisodeParagraph, VisualSelection } from "../lib/types";
import { LegacyMigrationPanel } from "./LegacyMigrationPanel";

type Tab = "script" | "story" | "picture" | "delivery";
const TABS: { id: Tab; label: string; hint: string }[] = [
  { id: "script", label: "文稿", hint: "通读与编辑" },
  { id: "story", label: "叙事方案", hint: "主线与人物" },
  { id: "picture", label: "证据选片", hint: "原文与必保" },
  { id: "delivery", label: "试听出片", hint: "声音与成片" },
];
const clock = (seconds: number) => `${Math.floor(seconds / 60)}:${Math.floor(seconds % 60).toString().padStart(2, "0")}`;

export function EpisodePanel({ taskId, disabled, onRenderedChange }: { taskId: string; disabled?: boolean; onRenderedChange?: (count: number) => void }) {
  const callback = useRef(onRenderedChange);
  callback.current = onRenderedChange;
  const alive = useRef(true);
  const flight = useRef(false);
  const feedbackRef = useRef<HTMLDivElement>(null);
  const [episode, setEpisode] = useState<Episode | null>(null);
  const [draft, setDraft] = useState<EpisodeScript | null>(null);
  const [tab, setTab] = useState<Tab>("script");
  const [candidatePath, setCandidatePath] = useState("");
  const [acceptStoryChange, setAcceptStoryChange] = useState(false);
  const [paragraph, setParagraph] = useState(0);
  const [busy, setBusy] = useState("");
  const [elapsed, setElapsed] = useState(0);
  const [migrating, setMigrating] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [reload, setReload] = useState(0);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [regenerateConfirm, setRegenerateConfirm] = useState(false);
  const [audioPreview, setAudioPreview] = useState("");
  const [sceneResult, setSceneResult] = useState<{ cut_candidates: number[]; warnings: string[] } | null>(null);
  const [storyEdit, setStoryEdit] = useState("");
  const [shotQuery, setShotQuery] = useState("");
  const [pendingOwner, setPendingOwner] = useState<EpisodeParagraph["audio_owner"]>();
  const [removal, setRemoval] = useState<number | null>(null);

  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => {
    let active = true;
    setLoading(true); setLoadFailed(false); setEpisode(null); setDraft(null); setError(""); setMessage("");
    setConfirmed(false); setAudioPreview(""); setSceneResult(null); setStoryEdit(""); setRegenerateConfirm(false); setParagraph(0);
    loadEpisode(taskId).then(value => { if (active) { setEpisode(value); setDraft(value?.plan ?? null); } })
      .catch(e => { if (active) { setError(String(e)); setLoadFailed(true); } })
      .finally(() => { if (active) setLoading(false); });
    const subscription = listen<{ task_id: string; message: string }>("episode-progress", e => {
      if (active && e.payload.task_id === taskId) setMessage(e.payload.message);
    });
    void subscription.catch(() => { if (active) setMessage("进度通知连接不可用，操作结果仍会返回；请勿重复提交。"); });
    return () => { active = false; void subscription.then(unlisten => unlisten()).catch(() => {}); };
  }, [taskId, reload]);
  useEffect(() => { if (!loading && !loadFailed) callback.current?.(episode?.output_path ? 1 : 0); }, [episode?.output_path, loading, loadFailed]);
  useEffect(() => {
    if (!busy) { setElapsed(0); return; }
    const start = Date.now(); const timer = window.setInterval(() => setElapsed(Math.floor((Date.now() - start) / 1000)), 1000);
    return () => window.clearInterval(timer);
  }, [busy]);
  useEffect(() => { setPendingOwner(undefined); setRemoval(null); setShotQuery(""); }, [paragraph]);

  const dirty = Boolean(draft && episode && JSON.stringify(draft) !== JSON.stringify(episode.plan));
  const legacy = episode?.schema_version === 1;
  const locked = Boolean(disabled || busy || migrating || loading || loadFailed);
  const reviewed = Boolean(episode && !legacy && episode.reviewed_revision === episode.revision && episode.audience_review?.verdict === "pass" && !dirty);
  const audiencePending = Boolean(episode && !legacy && !dirty && episode.reviewed_revision === episode.revision && !episode.audience_review);
  const repairPending = Boolean(!dirty && (episode?.review_error?.includes("[repair_response]") || episode?.review_error?.includes("$.narrations.") && episode?.review_error?.includes("maxLength")));
  const reviewPending = Boolean(audiencePending || repairPending || !dirty && ["[review_service]", "事实评审未覆盖全文所有段落", "全文评审结果格式无效"].some(marker => episode?.review_error?.includes(marker)));
  const coverageNeedsEdit = Boolean(!dirty && episode?.review_error?.includes("覆盖修复已尝试"));
  const recoveryExhausted = Boolean(!dirty && (episode?.review_error?.includes("自动恢复已用尽") || episode?.review_error?.includes("恢复稿继续修复已结束")));
  const canResumeBudget = Boolean(recoveryExhausted && !episode?.review_error?.includes("恢复稿继续修复已结束") && episode?.review_error?.includes("[picture_reserve]"));
  const chars = draft?.paragraphs.reduce((sum, p) => sum + Array.from(p.narration).length, 0) ?? 0;
  const sharedBlocker = loading ? "正在读取稿件" : loadFailed ? "读取失败，请刷新任务" : disabled ? "素材正在准备" : migrating ? "旧稿正在迁移" : busy ? `正在${busy}` : "";
  const reviewBlocker = sharedBlocker || (!episode ? "先生成整集稿" : legacy ? "旧稿需先迁移或重新生成" : reviewPending ? "" : recoveryExhausted && !canResumeBudget ? "自动恢复已用尽，请修改稿件后再审或重新生成" : coverageNeedsEdit ? "叙事节点仍缺证据，请修改问题段或重新生成" : "");
  const phase = legacy ? "旧稿待升级" : !episode ? "等待生成" : dirty ? "有未保存修改" : reviewed ? "AI审稿通过" : reviewPending ? "AI审稿待补全" : "待AI审稿";

  function accept(next: Episode) {
    setEpisode(next); setDraft(next.plan); setConfirmed(false); setAudioPreview(""); setPendingOwner(undefined); setRemoval(null);
  }
  async function action(label: string, work: () => Promise<void>) {
    if (flight.current) return;
    flight.current = true; setBusy(label); setError(""); setConfirmed(false); setMessage(`${label}请求已发出，正在等待后端确认…`);
    feedbackRef.current?.scrollIntoView({ behavior: "smooth", block: "nearest" });
    try { await work(); }
    catch (e) { if (alive.current) { setError(`${label}未完成：${String(e)}`); setMessage("操作已结束，请查看错误；未自动重试。"); } }
    finally { flight.current = false; if (alive.current) setBusy(""); }
  }
  async function review() {
    if (!episode || !draft || reviewBlocker) return;
    const snapshot = episode; const edited = draft;
    await action(repairPending ? "继续AI修复" : reviewPending ? "补全AI审稿" : "AI审查与自动修稿", async () => {
      let current = snapshot;
      if (dirty) {
        setMessage("先保存当前修改，保存成功后自动开始AI审稿…");
        current = await saveEpisode(taskId, edited, snapshot.revision);
        if (!alive.current) return;
        accept(current); setMessage("草稿已保存，正在提交AI审稿请求…");
      }
      const next = await reviewEpisode(taskId, current.revision);
      if (!alive.current) return;
      accept(next);
      const passed = next.reviewed_revision === next.revision;
      const incomplete = ["[review_service]", "事实评审未覆盖全文所有段落", "全文评审结果格式无效"].some(marker => next.review_error?.includes(marker));
      setMessage(passed ? "审稿完成 · 当前版本已通过。请通读后前往“试听出片”。" : incomplete ? "AI评审尚未完整返回 · 稿件已保存，可继续“补全审稿”，无需因此修改文案。" : "审稿完成 · 候选已保存，仍有待核问题。展开下方结果查看，不会自动出片。");
      if (!passed) setError(next.review_error || "当前版本尚未通过完整评审。");
    });
  }
  function edit(plan: EpisodeScript) { setDraft(plan); setConfirmed(false); setAudioPreview(""); }
  function updateParagraph(index: number, value: EpisodeParagraph) {
    if (draft) edit({ ...draft, paragraphs: draft.paragraphs.map((p, i) => i === index ? value : p) });
  }
  function updateVisual(index: number, selection: VisualSelection) {
    const p = draft?.paragraphs[paragraph];
    if (p) updateParagraph(paragraph, { ...p, visual_selections: p.visual_selections.map((v, i) => i === index ? selection : v) });
  }
  function regenerate() {
    setRegenerateConfirm(false);
    void action("生成整集稿", async () => {
      const next = await generateEpisode(taskId, episode?.revision ?? null);
      if (alive.current) { accept(next); setParagraph(0); setMessage("整集稿已保存，请核对AI评审结果。"); }
    });
  }
  async function locateOutput() {
    if (!episode?.output_path) return;
    try { await revealInFinder(episode.output_path); }
    catch(e) { setError(`无法定位成片：${String(e)}`); }
  }
  const current = draft?.paragraphs[paragraph];
  const visibleShots = episode?.shots.filter(s => !shotQuery || s.text.toLowerCase().includes(shotQuery.toLowerCase()) || String(s.id) === shotQuery) ?? [];
  const renderBlocker = sharedBlocker || (!episode ? "请先生成整集稿" : legacy ? "请先升级旧稿" : dirty ? "修改尚未保存并重评" : !reviewed ? "当前版本未通过AI审稿" : !confirmed ? "请勾选人工通读确认" : "");

  return <section className="episode-studio" aria-label="整集长视频解说">
    <header className="studio-heading">
      <div><div className="eyebrow">EPISODE WORKSPACE</div><h3>把一集故事，讲得完整。</h3><p>统一叙事 · 原速选片 · 本机情感配音</p></div>
      <span className={`studio-badge ${reviewed ? "is-ready" : ""}`}>{phase}</span>
    </header>
    <div className="studio-metrics">
      <div><span>全文字数</span><strong>{chars.toLocaleString()}<small> 字</small></strong></div>
      <div><span>旁白估时</span><strong>{(chars / 240).toFixed(1)}<small> 分钟</small></strong></div>
      <div><span>目标预算</span><strong>{episode ? (episode.target_secs / 60).toFixed(1) : "30%"}<small>{episode ? " 分钟" : " 原片时长"}</small></strong></div>
      <div><span>输出规格</span><strong>16:9<small> 横屏</small></strong></div>
    </div>
    <div className="studio-toolbar">
      <button type="button" className="primary" title={reviewBlocker || "会调用DeepSeek核对并按需修稿"} disabled={Boolean(reviewBlocker)} onClick={() => void review()}>
        {busy === "AI审查与自动修稿" ? "审稿进行中…" : dirty ? "保存并开始AI审稿" : "开始AI审稿与修稿"}
      </button>
      <button type="button" disabled={locked || legacy || !dirty || !draft || !episode} onClick={() => episode && draft && void action("保存草稿", async () => {
        const next = await saveEpisode(taskId, draft, episode.revision); if (alive.current) { accept(next); setMessage("草稿已保存，旧评审已失效。下一步开始AI审稿。"); }
      })}>保存草稿</button>
      <button type="button" disabled={locked} onClick={() => episode ? setRegenerateConfirm(true) : regenerate()}>{episode ? "重新生成…" : "生成整集稿"}</button>
      {episode?.output_path && <button type="button" onClick={() => void locateOutput()}>在 Finder 中定位成片 ↗</button>}
      <span className="toolbar-hint">{reviewBlocker || (dirty ? "未保存修改会先保存，再审稿" : "按需修稿最多两轮 · 不自动出片")}</span>
    </div>
    <div ref={feedbackRef} className={`operation-feedback ${busy ? "is-running" : error ? "is-error" : ""}`} aria-busy={Boolean(busy)}>
      <div className="operation-heading"><span className={busy ? "activity-spinner" : "activity-dot"} aria-hidden="true" /><strong>{busy ? `${busy} · ${clock(elapsed)}` : error ? "需要处理" : "当前进度"}</strong></div>
      <p role="status" aria-live="polite">{message || (reviewBlocker || (episode ? "稿件已载入。修改后可直接“保存并开始AI审稿”。" : "素材准备好后，先生成整集解说全文。"))}</p>
      {busy && elapsed >= 15 && <p className="field-help">AI请求可能需要数分钟，正在等待结果，不代表已通过。不要重复点击或关闭应用；当前操作结束后会显示结果。</p>}
      {error && <p role="alert" className="error-text">{error}</p>}
      {loadFailed && <button type="button" onClick={() => setReload(n => n + 1)}>重新读取任务</button>}
    </div>
    {regenerateConfirm && <section className="studio-confirm" aria-label="确认重新生成全文">
      <strong>重新生成会调用AI并产生用量</strong><p>按当前素材重新梳理，成功后归档旧稿并替换全文。未保存修改会丢弃；旧媒体保留，不重新下载或转写。</p>
      <div className="button-row"><button type="button" className="primary" disabled={locked} onClick={regenerate}>确认重新生成</button><button type="button" disabled={locked} onClick={() => setRegenerateConfirm(false)}>取消</button></div>
    </section>}
    {episode && legacy && <LegacyMigrationPanel taskId={taskId} revision={episode.revision} disabledReason={sharedBlocker} onBusyChange={setMigrating} onMigrated={next => {
      setMigrating(false); accept(next); setMessage("迁移完成。请核对文稿和画面，再点击“开始AI审稿与修稿”。");
    }} onRegenerate={() => setRegenerateConfirm(true)} />}
    {episode?.review_error && !busy && <details className="studio-review-result"><summary>审稿结果 · 当前版本尚未通过</summary><p>{episode.review_error}</p></details>}
    {episode && !legacy && <details className="advanced-panel"><summary>恢复已审稿候选</summary><p>应用会核对片源、字幕、事实评审和独立观众理解检查；成功后仍需人工通读确认。</p><label>候选JSON文件路径<input value={candidatePath} disabled={locked} placeholder="粘贴已审稿候选包的绝对路径" onChange={e => setCandidatePath(e.target.value)} /></label><label><input type="checkbox" checked={acceptStoryChange} disabled={locked} onChange={e => setAcceptStoryChange(e.target.checked)} />我已阅读候选，接受其中的叙事方案与场景分组调整。</label><button type="button" disabled={locked || dirty || !candidatePath.trim()} onClick={() => void action("恢复已审稿候选", async () => { const next = await importReviewedEpisode(taskId, episode.revision, candidatePath.trim(), acceptStoryChange); if (alive.current) { accept(next); setMessage("已恢复事实与观众理解检查均通过的候选。请通读并核对警告，再确认出片。"); } })}>校验并恢复候选</button>{dirty && <p className="field-help">请先保存或撤销未保存修改，再恢复候选。</p>}</details>}
    {audiencePending && <section className="studio-confirm"><strong>历史稿尚未检查观众能否听懂</strong><p>这份稿此前的事实评审通过，但还需检查开场、时间线、段落接续和中文口播；“事实有据”不能代替这一步。</p></section>}
    {coverageNeedsEdit && !reviewPending && <section className="studio-confirm"><strong>主线证据仍缺失</strong><p>定向修复已达到本轮上限。请修改问题段并保存，或点击“重新生成…”重写整集稿；原视频与字幕会复用。</p><button type="button" disabled={locked} onClick={() => setRegenerateConfirm(true)}>重新生成整集稿…</button></section>}
    {reviewPending && <section className="studio-confirm"><strong>{repairPending ? "AI修稿返回未满足要求" : "AI评审尚未完成"}</strong><p>{repairPending ? "当前稿件及评审已保留。可继续定向修复，程序会校验每段主线和字数预算，再做全文复审。" : "当前稿件已保留。评审返回遗漏或格式问题时可直接补全审稿，无需因此修改文案；发现漏评后程序会自动补评并核对全文。"}</p><button type="button" disabled={locked} onClick={() => void review()}>{repairPending ? "继续定向修复" : "补全审稿"}</button></section>}
    {recoveryExhausted && !reviewPending && <section className="studio-confirm"><strong>需要处理的异常任务</strong><p>{canResumeBudget ? "已归档的恢复稿仍有画面字数预算问题，可直接继续精简并全文复审，无需重写整集。" : "局部修复和一次自动全文恢复均已完成，稿件仍未通过。原稿与恢复稿均已归档；修改问题后可继续审稿。"}</p>{canResumeBudget ? <button type="button" disabled={locked} onClick={() => void review()}>继续修复恢复稿</button> : <button type="button" disabled={locked} onClick={() => setRegenerateConfirm(true)}>重新生成整集稿…</button>}</section>}
    {episode && draft && <>
      <nav className="studio-tabs" aria-label="工作台区域">{TABS.map(t => <button key={t.id} type="button" aria-current={tab === t.id ? "page" : undefined} className={tab === t.id ? "active" : ""} onClick={() => setTab(t.id)}>{t.label}<small>{t.hint}</small></button>)}</nav>
      {tab === "script" && <div className="studio-document">
        <label className="document-title">整集标题<input disabled={locked || legacy} value={draft.title} onChange={e => edit({ ...draft, title: e.target.value })} /></label>
        <div className="section-caption"><span>{draft.paragraphs.length} 个连续段落</span><span>{legacy ? "旧稿只读" : dirty ? "尚未保存" : "修改已保存"}</span></div>
        {draft.paragraphs.map((p, i) => <article className="document-paragraph" key={i}>
          <div className="paragraph-rail"><span>{String(i + 1).padStart(2, "0")}</span><small>{Array.from(p.narration).length}字</small></div>
          <div className="paragraph-content">
            <textarea aria-label={`第${i + 1}段文稿`} rows={Math.max(3, Math.min(7, Math.ceil(p.narration.length / 35)))} disabled={locked || legacy || (p.audio_owner !== undefined && p.audio_owner !== "narration")} value={p.narration} placeholder="此段由原声或有意留白主导，无旁白文字" onChange={e => updateParagraph(i, { ...p, narration: e.target.value })} />
            <div className="paragraph-footer"><span>{p.audio_owner === "original_dialogue" ? "原声对白" : p.audio_owner === "original_action" ? "动作原声" : p.audio_owner === "silence" ? "静音留白" : "旁白"} · {p.evidence_refs.length}条证据 · {p.visual_selections.length}个画面</span><button type="button" className="text-btn" onClick={() => { setParagraph(i); setTab("picture"); }}>查看证据与选片 →</button></div>
          </div>
        </article>)}
      </div>}
      {tab === "story" && <div className="studio-section">
        <div className="section-title"><h4>统一叙事方案</h4><span>方案是导航，原字幕才是事实依据</span></div>
        <p className="story-spine">{episode.story.spine}</p>
        {episode.story.narration_brief && <section className="studio-section"><h4>面向观众的讲述方案</h4><p><strong>开场情境：</strong>{episode.story.narration_brief.opening_context}</p><p><strong>时间线：</strong>{episode.story.narration_brief.timeline}</p><p><strong>贯穿主线：</strong>{episode.story.narration_brief.throughline}</p><details><summary>自然叙事单元</summary><ol>{episode.story.narration_brief.scenes.map((scene, i) => <li key={i}><p>{scene.context}</p><small>{scene.transition}</small></li>)}</ol></details></section>}
        <div className="story-grid"><section><h4>人物称呼</h4><ul>{episode.story.characters.map((person, i) => <li key={i}>{person}</li>)}</ul></section><section><h4>揭示与回收</h4><p>{episode.story.reveal_order}</p></section></div>
        <ol className="story-beats">{episode.story.beats.map((beat, i) => <li key={i}>{beat.task}</li>)}</ol>
        {!legacy && <details className="advanced-panel"><summary>高级：编辑方案JSON</summary><p>保存后需重评，不自动重写全文。</p><button disabled={locked || dirty} type="button" onClick={() => setStoryEdit(JSON.stringify(episode.story, null, 2))}>载入当前方案</button><textarea rows={12} disabled={locked || dirty} value={storyEdit} onChange={e => setStoryEdit(e.target.value)} /><button disabled={locked || dirty || !storyEdit} type="button" onClick={() => {
          try { const story = JSON.parse(storyEdit); void action("保存方案", async () => { const next = await saveEpisodeStory(taskId, episode.revision, episode.story, story); if (alive.current) { accept(next); setMessage("方案已保存，需重新审稿。"); } }); } catch { setError("方案JSON格式无效"); }
        }}>保存方案</button></details>}
      </div>}
      {tab === "picture" && current && <div className="studio-section">
        <div className="section-title"><h4>证据与画面</h4><select aria-label="选择编辑段落" value={paragraph} onChange={e => setParagraph(Number(e.target.value))}>{draft.paragraphs.map((_, i) => <option key={i} value={i}>第 {i + 1} 段</option>)}</select></div>
        <p className="selected-narration">{current.narration || "此段没有旁白，由原声或留白主导。"}</p>
        <div className="story-grid">
          <label>声音归属<select disabled={locked || legacy} value={current.audio_owner ?? "narration"} onChange={e => {
            const owner = e.target.value as NonNullable<EpisodeParagraph["audio_owner"]>;
            if (owner !== "narration" && current.narration) { setPendingOwner(owner); return; }
            updateParagraph(paragraph, { ...current, audio_owner: owner });
          }}><option value="narration">旁白主导</option><option value="original_dialogue">完整原声对白</option><option value="original_action">动作原声</option><option value="silence">有意静音留白</option></select></label>
          <label>连续原文引文<input disabled={locked || legacy} value={current.evidence_quote} onChange={e => updateParagraph(paragraph, { ...current, evidence_quote: e.target.value })} /></label>
        </div>
        {pendingOwner && <div className="studio-confirm"><p>改为原声或留白会清空本段配音文字，需保存重评；第一版仅支持一个连续源区间。</p><button disabled={locked} type="button" onClick={() => { updateParagraph(paragraph, { ...current, audio_owner: pendingOwner, narration: "" }); setPendingOwner(undefined); }}>确认切换并清空本段配音</button><button type="button" onClick={() => setPendingOwner(undefined)}>取消</button></div>}
        <p className="field-help">证据可跨段引用；画面不重复播放。无必保的画面可作为备用，出片时会按实测配音时长选择；必须出现的内容请标记必保区间。候选是字幕关联素材窗，不代表AI已观察画面，动作必保点请实际看片确认。</p>
        <label>筛选字幕或候选ID<input type="text" placeholder="输入原文关键词或ID" value={shotQuery} onChange={e => setShotQuery(e.target.value)} /></label>
        <div className="shot-picker">{visibleShots.slice(0, 100).map(shot => {
          const selection = current.visual_selections.find(v => v.candidate_id === shot.id);
          const taken = draft.paragraphs.some((p, i) => i !== paragraph && p.visual_selections.some(v => v.candidate_id === shot.id));
          return <div key={shot.id} className="shot-row"><span className="shot-time">#{shot.id}<small>{clock(shot.start)}–{clock(shot.end)}</small></span><p>{shot.text}</p><label><input type="checkbox" disabled={locked || legacy} checked={current.evidence_refs.includes(shot.id)} onChange={e => updateParagraph(paragraph, { ...current, evidence_refs: e.target.checked ? [...current.evidence_refs, shot.id].sort((a, b) => a - b) : current.evidence_refs.filter(id => id !== shot.id) })} />证据</label><label><input type="checkbox" disabled={locked || legacy || (!selection && taken)} checked={Boolean(selection)} onChange={e => {
            if (!e.target.checked && selection?.must_keep_ranges.length) { setRemoval(shot.id); return; }
            updateParagraph(paragraph, { ...current, visual_selections: e.target.checked ? [...current.visual_selections, { candidate_id: shot.id, start: shot.start, end: shot.end, must_keep_ranges: [] }].sort((a, b) => a.start - b.start) : current.visual_selections.filter(v => v.candidate_id !== shot.id) });
          }} />画面</label></div>;
        })}</div>
        {visibleShots.length > 100 && <p className="field-help">已显示前100条，请输入关键词缩小范围。</p>}
        {removal !== null && <div className="studio-confirm"><p>画面#{removal}含必保区间。移除会删除该保护，需重新评审。</p><button disabled={locked} type="button" onClick={() => { updateParagraph(paragraph, { ...current, visual_selections: current.visual_selections.filter(v => v.candidate_id !== removal) }); setRemoval(null); }}>确认移除</button><button type="button" onClick={() => setRemoval(null)}>取消</button></div>}
        {current.visual_selections.map((v, i) => <details className="advanced-panel" key={i}><summary>画面#{v.candidate_id} · {v.start.toFixed(2)}–{v.end.toFixed(2)}秒 · {v.must_keep_ranges.length}处必保</summary><div className="range-editor">{(["start", "end"] as const).map(field => <label key={field}>画面{field === "start" ? "开始" : "结束"}秒<input type="number" step="0.01" disabled={locked || legacy} value={v[field]} onChange={e => updateVisual(i, { ...v, [field]: Number(e.target.value) })} /></label>)}</div>
          {v.must_keep_ranges.map((anchor, ai) => <div className="range-editor" key={ai}>{(["start", "end"] as const).map(field => <label key={field}>必保{field === "start" ? "开始" : "结束"}秒<input type="number" step="0.01" disabled={locked || legacy} value={anchor[field]} onChange={e => updateVisual(i, { ...v, must_keep_ranges: v.must_keep_ranges.map((a, j) => j === ai ? { ...a, [field]: Number(e.target.value) } : a) })} /></label>)}<button type="button" disabled={locked || legacy} onClick={() => updateVisual(i, { ...v, must_keep_ranges: v.must_keep_ranges.filter((_, j) => j !== ai) })}>移除保护</button></div>)}
          <button type="button" disabled={locked || legacy} onClick={() => updateVisual(i, { ...v, must_keep_ranges: [...v.must_keep_ranges, { start: v.start, end: Math.min(v.end, v.start + 1) }] })}>添加人工必保区间</button>
        </details>)}
        <details className="advanced-panel"><summary>辅助：本机切点检测</summary><p>非模型像素算法，切点仅供参考，不自动更改对白或必保边界。</p><button type="button" disabled={locked} onClick={() => void action("本机切点检测", async () => { const result = await detectEpisodeScenes(taskId); if (alive.current) { setSceneResult(result); setMessage(`切点检测完成，共${result.cut_candidates.length}个候选。`); } })}>检测镜头切点候选</button>{sceneResult && <><p>{sceneResult.warnings.join("；")}</p><p>{sceneResult.cut_candidates.map(t => t.toFixed(3)).join("、")} 秒</p></>}</details>
      </div>}
      {tab === "delivery" && <div className="studio-section">
        <div className="delivery-grid"><section className="delivery-card"><span className="step-number">01</span><h4>先听，再渲染</h4><p>完整旁白试听，不含原声和留白占位。Qwen首次合成可能较慢，成功音频会缓存。</p><button type="button" disabled={locked || legacy || dirty} onClick={() => void action("完整旁白试听", async () => { const path = await previewEpisodeNarration(taskId, episode.revision); if (alive.current) { setAudioPreview(path); setMessage("旁白试听已生成，尚未渲染视频。"); } })}>{busy === "完整旁白试听" ? "正在合成旁白…" : "生成完整旁白试听"}</button>{audioPreview && <audio controls src={convertFileSrc(audioPreview)} />}</section>
          <section className="delivery-card"><span className="step-number">02</span><h4>确认后出片</h4><p>1920 × 1080 · 原速画面 · 实测配音时钟</p><label className="confirmation-line"><input type="checkbox" disabled={locked || !reviewed} checked={confirmed} onChange={e => setConfirmed(e.target.checked)} />我已通读当前全文并核对警告，确认出片。</label><button type="button" className="primary" title={renderBlocker} disabled={Boolean(renderBlocker)} onClick={() => void action("横屏出片", async () => { const next = await renderEpisode(taskId, episode.revision, true); if (alive.current) { accept(next); setMessage("横屏视频已生成，仍需完整看片与听审。"); } })}>{busy === "横屏出片" ? "正在生成视频…" : "生成整集横屏视频"}</button>{renderBlocker && <p className="field-help">{renderBlocker}</p>}</section></div>
        {episode.output_path && <section className="final-preview"><div className="section-title"><h4>当前成片</h4><span>实测 {((episode.duration_secs ?? 0) / 60).toFixed(1)} 分钟 · 待人工验收</span></div><video src={convertFileSrc(episode.output_path)} key={episode.output_path} controls preload="metadata" /><p className="file-path" style={{ overflowWrap: "anywhere" }}>保存位置：{episode.output_path}</p><button type="button" onClick={() => void locateOutput()}>在 Finder 中定位成片</button></section>}
      </div>}
      <details className="studio-footnotes"><summary>运行说明、警告与联网来源</summary><p>原语言字幕与稿件发送到配置的DeepSeek；可选Tavily仅查关键词，网页不是本集证据。没有完整评审通过不出片。配音和媒体处理在本机，字幕按实测时长估算，非强制对齐。</p>{episode.warnings.map((w, i) => <p key={i}>{w}</p>)}{episode.research?.map((r, i) => <div key={i}><p>{r.query.query} · {r.query.reason}</p>{r.error && <p>{r.error}</p>}{r.sources.map((s, j) => <p key={j}>{s.title} · {s.url}</p>)}</div>)}</details>
    </>}
    {!episode && !loading && !loadFailed && <div className="studio-empty"><span>01 → 02 → 03</span><h4>素材已就绪，从一份完整的故事开始</h4><p>生成全集方案和连续文稿，再审稿、试听、出片。</p></div>}
  </section>;
}
