import { useEffect, useState } from "react";
import { Settings } from "./components/Settings";
import { TaskQueueList } from "./components/TaskQueueList";
import { IngestProgress } from "./components/StatusStepper";
import { EpisodeWorkspace } from "./components/EpisodeWorkspace";
import { VideoPreview } from "./components/VideoPreview";
import { TranscriptViewer } from "./components/TranscriptViewer";
import { QueueFullError, useTaskQueue } from "./hooks/useTaskQueue";
import { getSettings } from "./lib/api";
import { STYLE_HINTS, STYLE_OPTIONS } from "./lib/types";
import {
  isValidVideoUrl,
  normalizeVideoUrl,
  videoUrlErrorMessage,
} from "./lib/videoUrl";

export default function App() {
  const [url, setUrl] = useState("");
  const [style, setStyle] = useState<string>(STYLE_OPTIONS[0]);
  const [formError, setFormError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const queue = useTaskQueue();
  const selected = queue.tasks.find((t) => t.id === selectedId) ?? null;

  useEffect(() => {
    getSettings().then((cfg) => {
      if (cfg.default_style) setStyle(cfg.default_style);
    });
  }, []);

  useEffect(() => {
    if (selectedId && queue.tasks.some((t) => t.id === selectedId)) return;
    const pick =
      queue.tasks.find((t) => t.status === "running") ||
      queue.tasks.find((t) => t.status === "ready") ||
      queue.tasks[0];
    if (pick) setSelectedId(pick.id);
  }, [queue.tasks, selectedId]);

  function handleStart() {
    const normalizedUrl = normalizeVideoUrl(url);
    if (!isValidVideoUrl(normalizedUrl)) {
      setFormError(videoUrlErrorMessage());
      return;
    }
    try {
      const id = queue.enqueuePrepare(normalizedUrl, style);
      setUrl("");
      setFormError(null);
      setSelectedId(id);
    } catch (e) {
      setFormError(e instanceof QueueFullError ? e.message : String(e));
    }
  }

  const selectedPrepare = selected?.prepare;
  return (
    <div className="app">
      <header className="app-header">
        <div className="brand">
          <div className="brand-icon">解</div>
          <div>
            <div className="eyebrow">VIDEO COMMENTARY STUDIO</div>
            <h1>AI 整集解说</h1>
            <p className="tagline">INGEST · FULL STORY · REVIEW · 16:9 RENDER</p>
          </div>
        </div>
        <div className="header-system">
          <span className="system-dot" />
          <div>
            <strong>AI DIRECTOR</strong>
            <span>DeepSeek API · 字幕与文案文本会发送到配置的服务</span>
          </div>
        </div>
      </header>

      <main className="app-layout">
        <section className="work-panel queue-panel">
          <div className="panel-content">
            <div className="eyebrow">01 · INGEST</div>
            <div className="panel-intro">
              <h2>创建解说任务</h2>
              <p>粘贴链接后，应用自动完成下载和字幕；随后统一梳理整集故事、写一篇连续稿，审稿后生成约原片30%时长的横屏解说。</p>
            </div>

            <div className="create-card">
              <label>
                视频链接
                <input
                  type="url"
                  className="input-lg"
                  placeholder="粘贴 YouTube、Bilibili、一帆、努努影院或其他视频链接"
                  value={url}
                  onChange={(e) => setUrl(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") handleStart();
                  }}
                />
              </label>

              <label>
                解说风格
                <select value={style} onChange={(e) => setStyle(e.target.value)}>
                  {STYLE_OPTIONS.map((s) => (
                    <option key={s} value={s}>
                      {s}
                    </option>
                  ))}
                </select>
              </label>
              {style in STYLE_HINTS && (
                <p className="style-note">{STYLE_HINTS[style as keyof typeof STYLE_HINTS]}</p>
              )}
            </div>

            {formError && <p className="queue-hint queue-full">{formError}</p>}
            {queue.queueError && (
              <p className="queue-hint queue-full">{queue.queueError}</p>
            )}

            <div className="button-row">
              <button
                type="button"
                className="primary"
                onClick={handleStart}
                disabled={queue.isQueueFull}
              >
                下载并准备素材
              </button>
            </div>

            <TaskQueueList
              tasks={queue.tasks}
              runningCount={queue.runningCount}
              queuedCount={queue.queuedCount}
              selectedId={selected?.id ?? null}
              onSelect={(task) => setSelectedId(task.id)}
              onRetry={queue.retryFailedTask}
              onRemove={queue.removeTask}
              onClearFinished={queue.clearFinished}
            />
          </div>
        </section>

        <section className="work-panel review-column">
          <div className="panel-content">
            {selected && (selected.status === "queued" || selected.status === "running") ? (
              <IngestProgress status={selected.status} progress={selected.progress} />
            ) : selected?.status === "failed" ? (
              <section className="review-panel">
                <div className="panel-intro">
                  <h2>任务执行失败</h2>
                  <p>{selected.error ?? "未知错误"}</p>
                </div>
                <div className="button-row">
                  <button
                    type="button"
                    className="primary"
                    onClick={() => void queue.retryFailedTask(selected.id)}
                  >
                    {selectedPrepare?.source_available ? "重试字幕准备" : "重新下载"}
                  </button>
                </div>
                {selectedPrepare?.source_available && (
                  <VideoPreview sourcePath={selectedPrepare.source_path} />
                )}
                {selectedPrepare?.transcript_available && (
                  <TranscriptViewer taskId={selectedPrepare.task_id} />
                )}
                {!selectedPrepare?.source_available && selected.url && (
                  <p className="queue-hint">重试后会使用原始播放链接重新下载视频。</p>
                )}
              </section>
            ) : selected && selectedPrepare ? (
              <EpisodeWorkspace
                prepare={selectedPrepare}
                status={selected.status}
                progress={selected.progress}
                onRefetchSubtitle={(taskId) => void queue.refetchSubtitleTask(taskId)}
                onRenderedChange={(rendered) => queue.setShortsRendered(selected.id, rendered)}
              />
            ) : (
              <div className="online-task-empty">
                <div className="empty-mark">⌁</div>
                <h2>解说工作台待命</h2>
                <p>选择左侧任务，生成整集连续解说稿，通读审稿后生成横屏长视频。</p>
              </div>
            )}
          </div>
        </section>
      </main>

      <Settings />
    </div>
  );
}
