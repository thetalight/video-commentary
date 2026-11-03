import { revealPath } from "../lib/api";
import type { QueueTaskStatus } from "../hooks/taskQueueShared";
import type { PrepareResult, TaskProgress } from "../lib/types";
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

      {prepare.excluded_ranges.length > 0 && (
        <div className="skip-summary">
          <span className="skip-summary-label">选片时跳过的区间</span>
          <div className="skip-chip-row">
            {prepare.excluded_ranges.map((range, index) => (
              <span className="skip-chip" key={`${range.kind}-${range.start}-${index}`}>
                {range.kind} {formatTime(range.start)}–{formatTime(range.end)}
              </span>
            ))}
          </div>
        </div>
      )}

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
