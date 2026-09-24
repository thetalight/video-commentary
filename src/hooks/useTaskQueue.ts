import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  deleteJob,
  generateDirectorPlan,
  listExistingJobs,
  prepareJob,
  refetchPlatformSubtitleAndGenerate,
  renderJob,
} from "../lib/api";
import {
  MAX_QUEUE_SIZE,
  type PrepareResult,
  type RenderResult,
  type TaskCompletePayload,
  type TaskProgress,
} from "../lib/types";
import {
  QueueFullError,
  newQueueTaskId,
  type QueueTaskStatus,
} from "./taskQueueShared";

export type TaskKind = "prepare" | "render";

export interface CommentaryTask {
  id: string;
  kind: TaskKind;
  label: string;
  url?: string;
  style?: string;
  status: QueueTaskStatus;
  progress: TaskProgress | null;
  prepare?: PrepareResult;
  render?: RenderResult;
  outputPath?: string;
  error?: string;
  restored?: boolean;
  failedStage?: string;
  errorCode?: string;
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

  useEffect(() => {
    const unProgress = listen<TaskProgress>("task-progress", (event) => {
      const taskId = event.payload.task_id;
      if (!taskId) return;
      syncTasks((prev) =>
        prev.map((task) =>
          task.id === taskId ? { ...task, progress: event.payload } : task,
        ),
      );
    });

    const unComplete = listen<TaskCompletePayload>("task-complete", (event) => {
      const taskId = event.payload.task_id;
      if (!taskId) return;
      syncTasks((prev) =>
        prev.map((task) =>
          task.id === taskId && event.payload.output_path.toLowerCase().endsWith(".mp4")
            ? { ...task, outputPath: event.payload.output_path }
            : task,
        ),
      );
    });

    return () => {
      unProgress.then((fn) => fn());
      unComplete.then((fn) => fn());
    };
  }, [syncTasks]);

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
              kind:
                job.status === "completed" || ["tts", "render", "burn"].includes(job.stage)
                  ? "render"
                  : "prepare",
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
              outputPath: job.output_path ?? undefined,
              error: job.error ?? undefined,
              failedStage: job.stage || undefined,
              errorCode: job.error_code ?? undefined,
              restored: true,
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

  const pumpQueue = useCallback(() => {
    // Dispatch queued commands immediately. The Rust task manager owns the
    // actual resource limits (download 2, AI 2, subtitle OCR 1, render 1), so the UI is
    // only a projection and never the concurrency authority.
    const queued = tasksRef.current.filter((task) => task.status === "queued");

    for (const task of queued) {
      runningIdsRef.current.add(task.id);
      syncTasks((prev) =>
        prev.map((t) =>
          t.id === task.id
            ? { ...t, status: "running", progress: null, error: undefined }
            : t,
        ),
      );

      void (async () => {
        try {
          if (task.kind === "prepare" && task.url) {
            const result = await prepareJob(
              task.url,
              task.id,
              task.style,
            );
            syncTasks((prev) =>
              prev.map((t) =>
                t.id === task.id
                  ? {
                      ...t,
                      status: result.waiting_for_director ? "awaiting_director" : "ready",
                      prepare: result,
                      label: result.title || t.label,
                    }
                  : t,
              ),
            );
          } else if (task.kind === "render") {
            const result = await renderJob(task.id);
            syncTasks((prev) =>
              prev.map((t) =>
                t.id === task.id
                  ? {
                      ...t,
                      status: "completed",
                      render: result,
                      outputPath: result.output_path,
                      label: result.title || t.label,
                    }
                  : t,
              ),
            );
          } else {
            throw new Error("任务参数不完整");
          }
        } catch (error) {
          const current = tasksRef.current.find((candidate) => candidate.id === task.id);
          syncTasks((prev) =>
            prev.map((t) =>
              t.id === task.id
                ? {
                    ...t,
                    status: "failed",
                    error: String(error),
                    failedStage:
                      current?.progress?.step ?? (task.kind === "render" ? "render" : "download"),
                  }
                : t,
            ),
          );
        } finally {
          runningIdsRef.current.delete(task.id);
          pumpQueue();
        }
      })();
    }
  }, [syncTasks]);

  const enqueuePrepare = useCallback(
    (url: string, style: string) => {
      if (liveQueueSize(tasksRef.current) >= MAX_QUEUE_SIZE) {
        setQueueError(new QueueFullError().message);
        throw new QueueFullError();
      }
      setQueueError(null);
      const task: CommentaryTask = {
        id: newQueueTaskId("job"),
        kind: "prepare",
        label: truncateUrl(url),
        url: url.trim(),
        style,
        status: "queued",
        progress: null,
      };
      syncTasks((prev) => [...prev, task]);
      pumpQueue();
      return task.id;
    },
    [pumpQueue, syncTasks],
  );

  const enqueueRender = useCallback(
    (prepare: PrepareResult) => {
      if (runningIdsRef.current.has(prepare.task_id)) return;
      syncTasks((prev) =>
        prev.map((t) =>
          t.id === prepare.task_id
            ? {
                ...t,
                kind: "render",
                status: "queued",
                progress: null,
                error: undefined,
                prepare,
                restored: false,
              }
            : t,
        ),
      );
      pumpQueue();
    },
    [pumpQueue, syncTasks],
  );

  const retryFailedTask = useCallback(
    async (taskId: string) => {
      const original = tasksRef.current.find((task) => task.id === taskId);
      if (!original || original.status !== "failed" || runningIdsRef.current.has(taskId)) {
        return;
      }

      const failedStage = original.failedStage ?? "";
      const isRenderStage = ["tts", "render", "burn"].includes(failedStage);
      const retryStep = isRenderStage
        ? failedStage
        : failedStage === "director"
          ? "director"
          : failedStage === "download_subtitle"
            ? "download_subtitle"
            : failedStage === "transcribe"
              ? "transcribe"
              : original.prepare?.transcript_available
                ? "director"
                : original.prepare?.source_available
                  ? "transcribe"
                  : "download";

      runningIdsRef.current.add(taskId);
      syncTasks((prev) =>
        prev.map((task) =>
          task.id === taskId
            ? {
                ...task,
                kind: isRenderStage ? "render" : "prepare",
                status: "running",
                restored: false,
                progress: {
                  task_id: taskId,
                  step: retryStep,
                  percent: 0,
                  message: "正在从失败阶段重试...",
                },
                error: undefined,
              }
            : task,
        ),
      );

      try {
        if (isRenderStage && original.prepare?.plan) {
          const result = await renderJob(taskId);
          syncTasks((prev) =>
            prev.map((task) =>
              task.id === taskId
                ? {
                    ...task,
                    kind: "render",
                    status: "completed",
                    render: result,
                    outputPath: result.output_path,
                    label: result.title || task.label,
                    progress: null,
                    failedStage: undefined,
                    errorCode: undefined,
                  }
                : task,
            ),
          );
          return;
        }

        let result: PrepareResult;
        if (failedStage === "download_subtitle" && original.prepare?.source_available) {
          result = await refetchPlatformSubtitleAndGenerate(taskId);
        } else if (failedStage === "transcribe" && original.prepare?.source_available) {
          result = await refetchPlatformSubtitleAndGenerate(taskId);
        } else if (
          (failedStage === "director" || original.prepare?.transcript_available) &&
          original.prepare?.transcript_available
        ) {
          result = await generateDirectorPlan(taskId);
        } else if (original.prepare?.source_available) {
          result = await refetchPlatformSubtitleAndGenerate(taskId);
        } else if (original.url) {
          result = await prepareJob(original.url, taskId, original.style);
        } else {
          throw new Error("该任务缺少原始视频和播放链接，无法自动重试下载");
        }

        syncTasks((prev) =>
          prev.map((task) =>
            task.id === taskId
              ? {
                  ...task,
                  kind: "prepare",
                  status: result.waiting_for_director ? "awaiting_director" : "ready",
                  prepare: result,
                  label: result.title || task.label,
                  progress: null,
                  failedStage: undefined,
                  errorCode: undefined,
                  error: result.director_error ?? undefined,
                }
              : task,
          ),
        );
      } catch (error) {
        const current = tasksRef.current.find((task) => task.id === taskId);
        syncTasks((prev) =>
          prev.map((task) =>
            task.id === taskId
              ? {
                  ...task,
                  status: "failed",
                  progress: null,
                  error: String(error),
                  failedStage: current?.progress?.step ?? retryStep,
                }
              : task,
          ),
        );
      } finally {
        runningIdsRef.current.delete(taskId);
      }
    },
    [syncTasks],
  );

  const regenerateDirector = useCallback(
    async (taskId: string) => {
      if (runningIdsRef.current.has(taskId)) return;
      runningIdsRef.current.add(taskId);
      syncTasks((prev) =>
        prev.map((task) =>
          task.id === taskId
            ? {
                ...task,
                kind: "prepare",
                status: "running",
                restored: false,
                outputPath: undefined,
                render: undefined,
                progress: {
                  task_id: taskId,
                  step: "director",
                  percent: 70,
                  message: "正在重新生成 AI 导演稿...",
                },
                error: undefined,
              }
            : task,
        ),
      );
      try {
        const result = await generateDirectorPlan(taskId);
        syncTasks((prev) =>
          prev.map((task) =>
            task.id === taskId
              ? {
                  ...task,
                  status: "ready",
                  prepare: result,
                  label: result.title || task.label,
                  progress: null,
                }
              : task,
          ),
        );
      } catch (error) {
        syncTasks((prev) =>
          prev.map((task) =>
            task.id === taskId
              ? {
                  ...task,
                  status: "awaiting_director",
                  progress: null,
                  error: String(error),
                  failedStage: "director",
                }
              : task,
          ),
        );
      } finally {
        runningIdsRef.current.delete(taskId);
      }
    },
    [syncTasks],
  );

  const refetchSubtitleTask = useCallback(
    async (taskId: string) => {
      if (runningIdsRef.current.has(taskId)) return;
      runningIdsRef.current.add(taskId);
      syncTasks((prev) =>
        prev.map((task) =>
          task.id === taskId
            ? {
                ...task,
                kind: "prepare",
                status: "running",
                restored: false,
                outputPath: undefined,
                render: undefined,
                progress: {
                  task_id: taskId,
                  step: "download",
                  percent: 0,
                  message: "正在重新获取最佳字幕...",
                },
                error: undefined,
              }
            : task,
        ),
      );
      try {
        const result = await refetchPlatformSubtitleAndGenerate(taskId);
        syncTasks((prev) =>
          prev.map((task) =>
            task.id === taskId
              ? {
                  ...task,
                  status: "ready",
                  prepare: result,
                  label: result.title || task.label,
                  progress: null,
                }
              : task,
          ),
        );
      } catch (error) {
        syncTasks((prev) =>
          prev.map((task) =>
            task.id === taskId
              ? {
                  ...task,
                  status: "failed",
                  progress: null,
                  error: String(error),
                  failedStage: "download_subtitle",
                }
              : task,
          ),
        );
      } finally {
        runningIdsRef.current.delete(taskId);
      }
    },
    [syncTasks],
  );

  const updatePrepare = useCallback(
    (taskId: string, prepare: PrepareResult) => {
      syncTasks((prev) =>
        prev.map((t) =>
          t.id === taskId
            ? {
                ...t,
                prepare,
                status: "ready",
                outputPath: undefined,
                render: undefined,
              }
            : t,
        ),
      );
    },
    [syncTasks],
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

  const runningCount = tasks.filter((task) => task.status === "running").length;
  const queuedCount = tasks.filter((task) => task.status === "queued").length;

  return {
    tasks,
    runningCount,
    queuedCount,
    isQueueFull: liveQueueSize(tasks) >= MAX_QUEUE_SIZE,
    queueError,
    enqueuePrepare,
    enqueueRender,
    retryFailedTask,
    regenerateDirector,
    refetchSubtitleTask,
    updatePrepare,
    removeTask,
    clearFinished,
  };
}
