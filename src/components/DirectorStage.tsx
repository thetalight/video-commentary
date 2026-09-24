import { revealPath } from "../lib/api";
import type { QueueTaskStatus } from "../hooks/taskQueueShared";
import type { PrepareResult, TaskProgress } from "../lib/types";
import { StatusStepper } from "./StatusStepper";
import { VideoPreview } from "./VideoPreview";
import { TranscriptViewer } from "./TranscriptViewer";

interface DirectorStageProps {
  prepare: PrepareResult;
  status: QueueTaskStatus;
  progress: TaskProgress | null;
  error?: string;
  onGenerate: (taskId: string) => void;
  onRefetchSubtitle: (taskId: string) => void;
}

function formatTime(seconds: number) {
  const minutes = Math.floor(seconds / 60);
  const remain = Math.round(seconds % 60);
  return `${minutes}:${remain.toString().padStart(2, "0")}`;
}

export function DirectorStage({
  prepare,
  status,
  progress,
  error,
  onGenerate,
  onRefetchSubtitle,
}: DirectorStageProps) {
  const message = error ?? prepare.director_error;
  const running = status === "running";

  return (
    <section className="review-panel director-stage">
      <div className="eyebrow">AI DIRECTOR</div>
      <div className="panel-intro">
        <h2>AI 导演需要处理</h2>
        <p>
          字幕已经准备好。请确认本机 Ollama 已启动且模型可用；应用会在本机重新执行“字幕理解 → 剧情节拍 → 导演脚本 → 整片评审”。
        </p>
      </div>

      <StatusStepper status={status} progress={progress} kind="prepare" />

      <VideoPreview sourcePath={prepare.source_path} />
      <TranscriptViewer taskId={prepare.task_id} source={prepare.subtitle_source} />

      {message && (
        <div className="director-alert">
          <strong>字幕或 AI 导演尚未完成</strong>
          <p>{message}</p>
        </div>
      )}

      {prepare.excluded_ranges.length > 0 && (
        <div className="skip-summary">
          <span className="skip-summary-label">已识别的跳过区间</span>
          <div className="skip-chip-row">
            {prepare.excluded_ranges.map((range, index) => (
              <span className="skip-chip" key={`${range.kind}-${range.start}-${index}`}>
                {range.kind} {formatTime(range.start)}–{formatTime(range.end)}
              </span>
            ))}
          </div>
        </div>
      )}

      <div className="director-path">
        <span>任务素材</span>
        <code>{prepare.inbox_dir}</code>
      </div>

      <div className="button-row">
        <button
          type="button"
          onClick={() => onRefetchSubtitle(prepare.task_id)}
          disabled={running}
        >
          重新获取最佳字幕
        </button>
        <button
          type="button"
          className="primary"
          onClick={() => onGenerate(prepare.task_id)}
          disabled={running}
        >
          {running ? "AI 生成中..." : "重试 AI 导演"}
        </button>
        <button type="button" onClick={() => revealPath(prepare.inbox_dir)}>
          打开任务资料
        </button>
      </div>
    </section>
  );
}
