import { useEffect, useState } from "react";
import { episodeExclusions, revealPath } from "../lib/api";
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
  const [exclusionError, setExclusionError] = useState("");
  async function refreshExclusions() {
    setExclusionError("");
    try { setExclusions(await episodeExclusions(prepare.task_id)); }
    catch { setExclusions(null); setExclusionError("无法读取当前排除报告，请刷新后再生成。旧素材准备时的范围不一定仍有效。"); }
  }
  useEffect(() => {
    let active = true;
    setExclusions(null); setExclusionError("");
    episodeExclusions(prepare.task_id).then(value => { if (active) setExclusions(value); })
      .catch(() => { if (active) setExclusionError("无法读取当前排除报告，请刷新后重试。"); });
    return () => { active = false; };
  }, [prepare.task_id, prepare.transcript_path]);

  return (
    <section className="review-panel director-stage">
      <div className="eyebrow">EPISODE DIRECTOR</div>
      <div className="panel-intro">
        <h2>{prepare.title}</h2>
        <p>
          原片和字幕（{prepare.subtitle_source}）已就绪。确认后端 DeepSeek API 已配置后，梳理整集并生成一篇连续解说稿，经事实和全文连贯性评审、人工审稿后生成16:9横屏长视频。
        </p>
      </div>

      <StatusStepper status={status} progress={progress} />

      <EpisodePanel
        key={prepare.task_id}
        taskId={prepare.task_id}
        disabled={running}
        onRenderedChange={onRenderedChange}
      />

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
          {exclusions.candidates.map((candidate,index) => <p key={index}>
            {candidate.applied ? "已按设置排除" : "候选未排除"} · {candidate.range.kind} · {formatTime(candidate.range.start)}–{formatTime(candidate.range.end)} · 字幕#{candidate.cue_index} · {candidate.reason}
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
    </section>
  );
}
