import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { deleteJob, listExistingJobs, prepareJob, refetchSubtitles } from "../lib/api";
import { MAX_QUEUE_SIZE, type PrepareResult, type TaskProgress } from "../lib/types";
import { QueueFullError, newQueueTaskId, type QueueTaskStatus } from "./taskQueueShared";

export interface CommentaryTask {
  id: string;
  label: string;
  url?: string;
  style?: string;
  status: QueueTaskStatus;
  progress: TaskProgress | null;
  prepare?: PrepareResult;
  error?: string;
  restored?: boolean;
  failedStage?: string;
  shortsRendered: number;
}

export { QueueFullError };

function liveQueueSize(tasks: CommentaryTask[]) {
  return tasks.filter((task) => !task.restored).length;
}

function truncateUrl(url: string, max = 56) {
  const trimmed = url.trim();
  if (trimmed.length <= max) return trimmed;
  return `${trimmed.slice(0, max - 1)}…`;
}

export function useTaskQueue() {
  const [tasks, setTasks] = useState<CommentaryTask[]>([]);
  const [queueError, setQueueError] = useState<string | null>(null);
  const tasksRef = useRef<CommentaryTask[]>([]);
  const runningIdsRef = useRef<Set<string>>(new Set());

  const syncTasks = useCallback((updater: (prev: CommentaryTask[]) => CommentaryTask[]) => {
    const next = updater(tasksRef.current);
    tasksRef.current = next;
    setTasks(next);
  }, []);

  const patch = useCallback(
    (taskId: string, change: (task: CommentaryTask) => Partial<CommentaryTask>) =>
      syncTasks((prev) =>
        prev.map((task) => (task.id === taskId ? { ...task, ...change(task) } : task)),
      ),
    [syncTasks],
  );

  useEffect(() => {
    const unProgress = listen<TaskProgress>("task-progress", (event) => {
      const taskId = event.payload.task_id;
      if (taskId) patch(taskId, () => ({ progress: event.payload }));
    });
    return () => {
      unProgress.then((fn) => fn());
    };
  }, [patch]);

  useEffect(() => {
    let cancelled = false;
    void listExistingJobs()
      .then((existing) => {
        if (cancelled) return;
        syncTasks((current) => {
          const activeIds = new Set(current.map((task) => task.id));
          const restored: CommentaryTask[] = existing
            .filter((job) => !activeIds.has(job.task_id))
            .map((job) => ({
              id: job.task_id,
              label: job.title,
              url: job.retry_url ?? undefined,
              status: job.status,
              progress:
                job.status === "running"
                  ? {
                      task_id: job.task_id,
                      step: job.stage,
                      percent: job.progress,
                      message: job.message,
                    }
                  : null,
              prepare: job.prepare,
              error: job.error ?? undefined,
              failedStage: job.stage || undefined,
              restored: true,
              shortsRendered: job.shorts_rendered,
            }));
          return [...restored, ...current];
        });
      })
      .catch((error) => {
        if (!cancelled) setQueueError(`读取既有任务失败：${String(error)}`);
      });
    return () => {
      cancelled = true;
    };
  }, [syncTasks]);

  /** Run one ingest command; the Rust side owns the real concurrency limits. */
  const run = useCallback(
    async (taskId: string, step: string, message: string, command: () => Promise<PrepareResult>) => {
      if (runningIdsRef.current.has(taskId)) return;
      runningIdsRef.current.add(taskId);
      patch(taskId, () => ({
        status: "running",
        restored: false,
        error: undefined,
        progress: { task_id: taskId, step, percent: 0, message },
      }));
      try {
        const result = await command();
        patch(taskId, (task) => ({
          status: task.shortsRendered > 0 ? "completed" : "ready",
          prepare: result,
          label: result.title || task.label,
          progress: null,
          failedStage: undefined,
        }));
      } catch (error) {
        patch(taskId, (task) => ({
          status: "failed",
          error: String(error),
          failedStage: task.progress?.step ?? step,
          progress: null,
        }));
      } finally {
        runningIdsRef.current.delete(taskId);
      }
    },
    [patch],
  );

  const enqueuePrepare = useCallback(
    (url: string, style: string) => {
      if (liveQueueSize(tasksRef.current) >= MAX_QUEUE_SIZE) {
        setQueueError(new QueueFullError().message);
        throw new QueueFullError();
      }
      setQueueError(null);
      const task: CommentaryTask = {
        id: newQueueTaskId("job"),
        label: truncateUrl(url),
        url: url.trim(),
        style,
        status: "queued",
        progress: null,
        shortsRendered: 0,
      };
      syncTasks((prev) => [...prev, task]);
      void run(task.id, "download", "正在启动...", () => prepareJob(task.url!, task.id, style));
      return task.id;
    },
    [run, syncTasks],
  );

  const refetchSubtitleTask = useCallback(
    (taskId: string) =>
      run(taskId, "download_subtitle", "正在重新获取字幕...", () => refetchSubtitles(taskId)),
    [run],
  );

  /** A downloaded film only needs its subtitles retried; otherwise start over from the link. */
  const retryFailedTask = useCallback(
    async (taskId: string) => {
      const task = tasksRef.current.find((candidate) => candidate.id === taskId);
      if (!task || task.status !== "failed") return;
      if (task.prepare?.source_available) {
        await refetchSubtitleTask(taskId);
      } else if (task.url) {
        await run(taskId, "download", "正在重新下载...", () =>
          prepareJob(task.url!, taskId, task.style),
        );
      } else {
        patch(taskId, () => ({ error: "该任务缺少原始视频和播放链接，无法自动重试下载" }));
      }
    },
    [patch, refetchSubtitleTask, run],
  );

  const setShortsRendered = useCallback(
    (taskId: string, rendered: number) =>
      patch(taskId, (task) => ({
        shortsRendered: rendered,
        status:
          task.status === "ready" || task.status === "completed"
            ? rendered > 0
              ? "completed"
              : "ready"
            : task.status,
      })),
    [patch],
  );

  const removeTask = useCallback(
    async (id: string) => {
      if (runningIdsRef.current.has(id)) return;
      setQueueError(null);
      try {
        await deleteJob(id);
        syncTasks((prev) => prev.filter((task) => task.id !== id));
      } catch (error) {
        setQueueError(`删除任务失败：${String(error)}`);
        throw error;
      }
    },
    [syncTasks],
  );

  const clearFinished = useCallback(() => {
    syncTasks((prev) =>
      prev.filter((task) => task.status !== "completed" && task.status !== "failed"),
    );
    setQueueError(null);
  }, [syncTasks]);

  return {
    tasks,
    runningCount: tasks.filter((task) => task.status === "running").length,
    queuedCount: tasks.filter((task) => task.status === "queued").length,
    isQueueFull: liveQueueSize(tasks) >= MAX_QUEUE_SIZE,
    queueError,
    enqueuePrepare,
    retryFailedTask,
    refetchSubtitleTask,
    setShortsRendered,
    removeTask,
    clearFinished,
  };
}
