import type { QueueTaskStatus } from "../hooks/taskQueueShared";
import type { TaskProgress } from "../lib/types";

const STEP_LABELS: Record<string, string> = {
  download: "下载视频",
  download_subtitle: "获取字幕",
  transcribe: "获取字幕",
};

const STEPS = [
  { id: "download", label: "下载" },
  { id: "subtitle", label: "字幕" },
  { id: "shorts", label: "整集" },
] as const;

function activeIndex(status: QueueTaskStatus, progress: TaskProgress | null): number {
  if (status === "failed") return -1;
  if (status === "ready") return 2;
  if (status === "completed") return STEPS.length;
  if (status === "running") {
    const step = progress?.step;
    return step === "transcribe" || step === "download_subtitle" ? 1 : 0;
  }
  return 0;
}

interface StatusStepperProps {
  status: QueueTaskStatus;
  progress: TaskProgress | null;
}

export function StatusStepper({ status, progress }: StatusStepperProps) {
  const active = activeIndex(status, progress);
  return (
    <ol className="status-stepper" aria-label="任务进度">
      {STEPS.map((step, i) => {
        const done = active === STEPS.length || i < active;
        const current = i === active;
        return (
          <li
            key={step.id}
            className={`status-step ${done ? "done" : ""} ${current ? "current" : ""}`}
          >
            <span className="status-dot" />
            <span className="status-step-label">{step.label}</span>
          </li>
        );
      })}
    </ol>
  );
}

export function IngestProgress({ status, progress }: StatusStepperProps) {
  return (
    <section className="review-panel">
      <div className="panel-intro">
        <h2>正在准备素材</h2>
        <p>
          下载视频并获取字幕：先找平台独立字幕，没有再找视频内封字幕；两种都没有就放弃这个视频。
        </p>
      </div>
      <StatusStepper status={status} progress={progress} />
      {progress ? (
        <div className="online-task-progress">
          <div className="progress-header">
            <span className="progress-step">
              {STEP_LABELS[progress.step] ?? progress.step}
            </span>
            <span className="progress-percent">{Math.round(progress.percent)}%</span>
          </div>
          <div className="progress-bar">
            <div className="progress-fill" style={{ width: `${progress.percent}%` }} />
          </div>
          {progress.message && <p className="progress-message">{progress.message}</p>}
        </div>
      ) : (
        <p className="progress-message">正在启动...</p>
      )}
    </section>
  );
}
