import { useState } from "react";
import type { CommentaryTask } from "../hooks/useTaskQueue";
import type { QueueTaskStatus } from "../hooks/taskQueueShared";

interface TaskQueueListProps {
  tasks: CommentaryTask[];
  runningCount: number;
  queuedCount: number;
  selectedId: string | null;
  onSelect: (task: CommentaryTask) => void;
  onRetry: (id: string) => void | Promise<void>;
  onRemove: (id: string) => void | Promise<void>;
  onClearFinished: () => void;
}

const STEP_LABELS: Record<string, string> = {
  download: "下载视频",
  download_subtitle: "拉取字幕",
  transcribe: "字幕获取",
  director: "AI 导演",
  tts: "配音",
  render: "剪辑拼接",
  burn: "烧录字幕",
};

const STATUS_LABELS: Record<QueueTaskStatus, string> = {
  queued: "排队中",
  running: "进行中",
  awaiting_director: "AI 待重试",
  ready: "待审稿",
  completed: "已成片",
  failed: "失败",
};

const KIND_LABELS: Record<string, string> = {
  prepare: "下载字幕",
  render: "生成成片",
};

export function TaskQueueList({
  tasks,
  runningCount,
  queuedCount,
  selectedId,
  onSelect,
  onRetry,
  onRemove,
  onClearFinished,
}: TaskQueueListProps) {
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const finishedCount = tasks.filter(
    (task) => task.status === "completed" || task.status === "failed",
  ).length;

  async function confirmDelete(taskId: string) {
    if (deletingId) return;
    setDeletingId(taskId);
    try {
      await onRemove(taskId);
      setPendingDeleteId(null);
    } catch {
      // The parent displays the backend error above the task list. Keep the
      // inline confirmation open so the user can retry after fixing it.
    } finally {
      setDeletingId(null);
    }
  }

  if (tasks.length === 0) {
    return (
      <div className="online-task-empty">
        <p>粘贴链接并开始后，任务会出现在这里。</p>
      </div>
    );
  }

  return (
    <section className="online-task-list">
      <div className="online-task-list-header">
        <h3>任务队列 · 既有任务</h3>
        <div className="online-task-list-actions">
          <span className="online-task-count">
            共 {tasks.length} 项 · 后端执行 {runningCount} · 待提交 {queuedCount}
          </span>
          {finishedCount > 0 && (
            <button type="button" className="text-btn" onClick={onClearFinished}>
              仅从页面隐藏
            </button>
          )}
        </div>
      </div>

      <ul className="online-task-items">
        {tasks.map((task) => (
          <li
            key={task.id}
            className={`online-task-card status-${task.status} ${
              selectedId === task.id ? "selected" : ""
            }`}
          >
            <div className="online-task-head">
              <button
                type="button"
                className="task-select-btn"
                onClick={() => onSelect(task)}
              >
                <span className={`task-status-badge badge-${task.status}`}>
                  {STATUS_LABELS[task.status]}
                </span>
                <span className="task-kind-badge">
                  {KIND_LABELS[task.kind] ?? task.kind}
                </span>
                {task.restored && <span className="task-history-badge">历史</span>}
                <strong className="online-task-label">{task.label}</strong>
              </button>
              <div className="task-card-actions">
                {task.status === "failed" && (
                  <button
                    type="button"
                    className="task-retry-btn"
                    onClick={() => void onRetry(task.id)}
                  >
                    重试
                  </button>
                )}
                {pendingDeleteId === task.id ? (
                  <>
                    <button
                      type="button"
                      className="task-confirm-delete-btn"
                      onClick={() => void confirmDelete(task.id)}
                      disabled={deletingId === task.id}
                    >
                      {deletingId === task.id ? "正在删除…" : "确认删除"}
                    </button>
                    <button
                      type="button"
                      className="task-cancel-delete-btn"
                      onClick={() => setPendingDeleteId(null)}
                      disabled={deletingId === task.id}
                    >
                      取消
                    </button>
                  </>
                ) : (
                  <button
                    type="button"
                    className="task-remove-btn"
                    onClick={() => setPendingDeleteId(task.id)}
                    disabled={task.status === "running" || deletingId !== null}
                    aria-label="删除任务及全部资源"
                    title={
                      task.status === "running"
                        ? "任务运行结束后才能删除"
                        : "从磁盘彻底删除任务及全部资源"
                    }
                  >
                    {task.status === "running" ? "运行中不可删" : "彻底删除"}
                  </button>
                )}
              </div>
            </div>

            {task.status === "awaiting_director" && (
              <p className="progress-message">检查本机 Ollama 服务和模型后重试</p>
            )}

            {task.status === "running" && (
              <div className="online-task-progress">
                {task.progress ? (
                  <>
                    <div className="progress-header">
                      <span className="progress-step">
                        {STEP_LABELS[task.progress.step] ?? task.progress.step}
                      </span>
                      <span className="progress-percent">
                        {Math.round(task.progress.percent)}%
                      </span>
                    </div>
                    <div className="progress-bar">
                      <div
                        className="progress-fill"
                        style={{ width: `${task.progress.percent}%` }}
                      />
                    </div>
                    {task.progress.message && (
                      <p className="progress-message">{task.progress.message}</p>
                    )}
                  </>
                ) : (
                  <p className="progress-message">正在启动...</p>
                )}
              </div>
            )}

            {task.status === "failed" && task.error && (
              <p className="online-task-error">{task.error}</p>
            )}

            {pendingDeleteId === task.id && (
              <p className="task-delete-warning">
                将永久删除原视频、字幕、导演稿、配音片段和成片，无法恢复。
              </p>
            )}

            {task.outputPath && task.status === "completed" && (
              <p className="file-path">{task.outputPath}</p>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}
