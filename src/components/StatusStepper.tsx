import type { QueueTaskStatus } from "../hooks/taskQueueShared";
import type { TaskProgress } from "../lib/types";

const STEP_LABELS: Record<string, string> = {
  download: "下载视频",
  transcribe: "字幕获取",
  director: "AI 导演",
  tts: "配音",
  render: "剪辑拼接",
  burn: "烧录字幕",
};

const STEPS = [
  { id: "download", label: "下载" },
  { id: "subtitle", label: "字幕" },
  { id: "director", label: "AI 导演" },
  { id: "review", label: "审稿" },
  { id: "render", label: "成片" },
] as const;

function activeIndex(
  status: QueueTaskStatus,
  progress: TaskProgress | null,
  kind?: string,
): number {
  if (status === "queued") return 0;
  if (status === "failed") return -1;
  if (status === "completed") return STEPS.length;
  if (status === "ready") return 3;
  if (status === "awaiting_director") return 2;
  if (status === "running") {
    if (kind === "render") return 4;
    const step = progress?.step;
    if (step === "transcribe") return 1;
    if (step === "director") return 2;
    if (step === "tts" || step === "render" || step === "burn") return 4;
    return 0;
  }
  return 0;
}

interface StatusStepperProps {
  status: QueueTaskStatus;
  progress: TaskProgress | null;
  kind?: string;
}

export function StatusStepper({ status, progress, kind }: StatusStepperProps) {
  const active = activeIndex(status, progress, kind);
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

export function IngestProgress({ status, progress, kind }: StatusStepperProps) {
  return (
    <section className="review-panel">
      <div className="panel-intro">
        <h2>正在准备素材</h2>
        <p>下载视频并抽取字幕，随后由已配置的 AI 模型梳理剧情并生成导演脚本。</p>
      </div>
      <StatusStepper status={status} progress={progress} kind={kind} />
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
