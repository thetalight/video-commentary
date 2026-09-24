import { MAX_QUEUE_SIZE } from "../lib/types";

export type QueueTaskStatus =
  | "queued"
  | "running"
  | "awaiting_director"
  | "completed"
  | "failed"
  | "ready";

export class QueueFullError extends Error {
  constructor() {
    super(`任务队列已满（最多 ${MAX_QUEUE_SIZE} 个），请先清除已完成任务`);
    this.name = "QueueFullError";
  }
}

export function newQueueTaskId(prefix: string) {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}
