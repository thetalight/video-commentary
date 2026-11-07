import { useEffect, useState } from "react";
import { episodeExclusions, saveEpisodeExclusions, revealPath } from "../lib/api";
import type { QueueTaskStatus } from "../hooks/taskQueueShared";
import type { ExclusionReport, PrepareResult, TaskProgress } from "../lib/types";
import { StatusStepper } from "./StatusStepper";
import { VideoPreview } from "./VideoPreview";
import { TranscriptViewer } from "./TranscriptViewer";
import { EpisodePanel } from "./EpisodePanel";

interface EpisodeWorkspaceProps {
  prepare: PrepareResult;
  status: QueueTaskStatus;
  progress: TaskProgress | null;
  onRefetchSubtitle: (taskId: string) => void;
  onRenderedChange: (rendered: number) => void;
}

function formatTime(seconds: number) {
  const minutes = Math.floor(seconds / 60);
  const remain = Math.round(seconds % 60);
  return `${minutes}:${remain.toString().padStart(2, "0")}`;
}

export function EpisodeWorkspace({
  prepare,
  status,
  progress,
  onRefetchSubtitle,
  onRenderedChange,
}: EpisodeWorkspaceProps) {
  const running = status === "running";
  const [exclusions, setExclusions] = useState<ExclusionReport | null>(null);
  const [exclusionChoices,setExclusionChoices]=useState<number[]>([]);
  const [savingExclusions,setSavingExclusions]=useState(false);
  const [exclusionError, setExclusionError] = useState("");
  async function refreshExclusions() {
    setExclusionError("");
    try { const report=await episodeExclusions(prepare.task_id); setExclusions(report);setExclusionChoices(report.candidates.flatMap((c,i)=>c.applied?[i]:[])); }
    catch { setExclusions(null); setExclusionError("无法读取当前排除报告，请刷新后再生成。旧素材准备时的范围不一定仍有效。"); }
  }
  useEffect(() => {
    let active = true;
    setExclusions(null); setExclusionError("");
    episodeExclusions(prepare.task_id).then(value => { if (active) {setExclusions(value);setExclusionChoices(value.candidates.flatMap((c,i)=>c.applied?[i]:[]));} })
      .catch(() => { if (active) setExclusionError("无法读取当前排除报告，请刷新后重试。"); });
    return () => { active = false; };
  }, [prepare.task_id, prepare.transcript_path]);

  return (
    <section className="review-panel episode-workspace">
      <div className="eyebrow">当前任务</div>
      <div className="panel-intro">
        <h2>{prepare.title}</h2>
        <p>
          {prepare.subtitle_source}已就绪 · 原片 {formatTime(prepare.duration_secs)} · 目标约 {formatTime(prepare.duration_secs * 0.3)}
        </p>
      </div>

      <StatusStepper status={status} progress={progress} />

      <EpisodePanel
        key={prepare.task_id}
        taskId={prepare.task_id}
        disabled={running}
        onRenderedChange={onRenderedChange}
      />

      <details className="source-tools"><summary>原片、字幕与排除设置 · 辅助工具</summary>
      <VideoPreview sourcePath={prepare.source_path} />
      <TranscriptViewer taskId={prepare.task_id} source={prepare.subtitle_source} />

      <details className="skip-summary">
        <summary>素材覆盖与候选排除（当前设置）</summary>
        <p>无可靠标记不裁片头片尾；音乐、完结提示或重复台词只作候选。当前只自动应用明确署名/广告标记对应的实际字幕区间，不扩展到开头、结尾或相邻剧情。需要保留这些区间时，请在设置关闭对应跳过选项后刷新；已有稿件需重新生成。</p>
        <button type="button" disabled={running} onClick={() => void refreshExclusions()}>刷新当前排除报告</button>
        {exclusionError && <p role="alert">{exclusionError}</p>}
        {exclusions && <>
          <p>原始 {exclusions.original_cue_count} 条 · 保留 {exclusions.retained_cue_positions.length} 条 · 排除 {exclusions.excluded_cue_positions.length} 条 · 规则 {exclusions.policy_version}</p>
          {exclusions.candidates.length === 0 && <p>没有候选排除区间，保留全部有效字幕材料。</p>}
          <button type="button" disabled={running||savingExclusions} onClick={async()=>{
            if (!window.confirm("确认这些候选排除选择？如果材料变化，旧稿将只能查看并需重新生成；原字幕不修改。"))return;
            setSavingExclusions(true);setExclusionError("");
            try{const report=await saveEpisodeExclusions(prepare.task_id,exclusions.candidate_fingerprint,exclusionChoices);setExclusions(report);setExclusionError("选择已保存。若材料变化请重新选择任务并生成整集，旧稿不自动覆盖。");}catch(e){setExclusionError(String(e));}finally{setSavingExclusions(false);}
          }}>确认逐区间排除选择</button>
          {exclusions.candidates.map((candidate,index) => <p key={index}>
            <input type="checkbox" disabled={running||savingExclusions} checked={exclusionChoices.includes(index)} onChange={e=>setExclusionChoices(current=>e.target.checked?[...current,index]:current.filter(i=>i!==index))} />{candidate.applied ? "当前已排除" : "当前保留"} · {candidate.range.kind} · {formatTime(candidate.range.start)}–{formatTime(candidate.range.end)} · 字幕#{candidate.cue_index} · {candidate.reason}
          </p>)}
        </>}
      </details>

      <div className="button-row">
        <button
          type="button"
          onClick={() => onRefetchSubtitle(prepare.task_id)}
          disabled={running}
        >
          {running ? "正在获取字幕..." : "重新获取字幕"}
        </button>
        <button type="button" onClick={() => revealPath(prepare.job_dir)}>
          打开任务目录
        </button>
      </div>
      </details>
    </section>
  );
}
