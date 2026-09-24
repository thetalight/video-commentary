import { useEffect, useState } from "react";
import { exportCommentaryScript, revealPath, saveEditPlan } from "../lib/api";
import type { QueueTaskStatus } from "../hooks/taskQueueShared";
import type { EditPlan, EditSegment, PrepareResult, TaskProgress } from "../lib/types";
import { StatusStepper } from "./StatusStepper";
import { VideoPreview } from "./VideoPreview";
import { TranscriptViewer } from "./TranscriptViewer";
import { OpeningSamplePanel } from "./OpeningSamplePanel";

interface ReviewPanelProps {
  prepare: PrepareResult;
  status: QueueTaskStatus;
  progress: TaskProgress | null;
  kind?: string;
  outputPath?: string;
  rendering?: boolean;
  onPlanSaved: (prepare: PrepareResult) => void;
  onRender: (prepare: PrepareResult) => void;
  onRegenerate: (taskId: string) => void;
  onRefetchSubtitle: (taskId: string) => void;
}

// Mirrors the backend picture-reserve rule in
// src-tauri/src/services/edit_timing.rs: pictures must outlast the estimated voice
// track (chars/4, floor 2.5s) by 1.2x, because rendering plays pictures at native
// speed and never pads a shortfall with a freeze or slow motion.
const CHARS_PER_SECOND = 4;
const MIN_SPOKEN_SECONDS = 2.5;
const PICTURE_RESERVE_RATIO = 1.2;
const RESERVE_EPSILON = 1e-6;

type ReserveStatus = "blocking" | "marginal" | "ok" | "handoff";

interface SegmentReserve {
  status: ReserveStatus;
  available: number;
  spoken: number;
  reserve: number;
  cutChars: number;
}

function analyzeReserve(segment: EditSegment): SegmentReserve {
  const chars = [...segment.narration].length;
  const spoken = Math.max(chars / CHARS_PER_SECOND, MIN_SPOKEN_SECONDS);
  const reserve = spoken * PICTURE_RESERVE_RATIO;
  const available = segment.shots?.length
    ? segment.shots.reduce(
        (total, [start, end]) => total + Math.max(end - start, 0),
        0,
      )
    : Math.max(segment.src_end - segment.src_start, 0);
  if (segment.keep_original_audio) {
    // Handoff paragraphs play the whole window; the backend checks them against
    // source dialogue instead of the multishot reserve rule.
    return { status: "handoff", available, spoken, reserve, cutChars: 0 };
  }
  const deficit =
    spoken > available + RESERVE_EPSILON
      ? spoken - available
      : reserve > available + RESERVE_EPSILON
        ? reserve - available
        : 0;
  const status: ReserveStatus =
    spoken > available + RESERVE_EPSILON
      ? "blocking"
      : reserve > available + RESERVE_EPSILON
        ? "marginal"
        : "ok";
  return {
    status,
    available,
    spoken,
    reserve,
    cutChars: Math.ceil(deficit * CHARS_PER_SECOND),
  };
}

export function ReviewPanel(props: ReviewPanelProps) {
  if (!props.prepare.plan) return null;
  return <ReviewPanelLoaded {...props} plan={props.prepare.plan} />;
}

function ReviewPanelLoaded({
  prepare,
  plan: initialPlan,
  status,
  progress,
  kind,
  outputPath,
  rendering,
  onPlanSaved,
  onRender,
  onRegenerate,
  onRefetchSubtitle,
}: ReviewPanelProps & { plan: EditPlan }) {
  const [plan, setPlan] = useState<EditPlan>(initialPlan);
  const [saving, setSaving] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [exportNotice, setExportNotice] = useState<string | null>(null);

  useEffect(() => {
    setPlan(initialPlan);
    setError(null);
    setExportNotice(null);
  }, [prepare.task_id, initialPlan]);

  function updateSegment(
    index: number,
    patch: Partial<EditPlan["segments"][number]>,
  ) {
    setPlan({
      ...plan,
      segments: plan.segments.map((seg, i) =>
        i === index ? { ...seg, ...patch } : seg,
      ),
    });
  }

  function removeSegment(index: number) {
    if (plan.segments.length <= 1) return;
    setPlan({
      ...plan,
      segments: plan.segments.filter((_, i) => i !== index),
    });
  }

  function addSegment() {
    const previous = plan.segments[plan.segments.length - 1];
    const start = previous?.src_end ?? 0;
    setPlan({
      ...plan,
      segments: [
        ...plan.segments,
        {
          src_start: start,
          src_end: start + 4,
          narration: "补充这一段的因果信息",
          keep_original_audio: false,
        },
      ],
    });
  }

  async function handleSave() {
    setSaving(true);
    setError(null);
    try {
      const saved = await saveEditPlan(prepare.task_id, plan);
      onPlanSaved({
        ...prepare,
        plan: saved,
        title: saved.title,
        waiting_for_director: false,
      });
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  async function handleRender() {
    if (blockingCount > 0) {
      setError(
        `有 ${blockingCount} 段画面盖不住口播（标红段落）。先扩选画面或按提示删字；渲染不会用定格或慢放补时。`,
      );
      return;
    }
    try {
      const saved = await saveEditPlan(prepare.task_id, plan);
      const next = {
        ...prepare,
        plan: saved,
        title: saved.title,
        waiting_for_director: false,
      };
      onPlanSaved(next);
      onRender(next);
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleExport() {
    setExporting(true);
    setError(null);
    setExportNotice(null);
    try {
      const saved = await saveEditPlan(prepare.task_id, plan);
      setPlan(saved);
      onPlanSaved({
        ...prepare,
        plan: saved,
        title: saved.title,
        waiting_for_director: false,
      });
      const result = await exportCommentaryScript(prepare.task_id);
      setExportNotice(
        result.srt_path
          ? "已导出解说文稿 TXT 和精确时间轴 SRT。"
          : "已导出解说文稿 TXT；生成过本版成片后，还会同时导出精确时间轴 SRT。",
      );
      await revealPath(result.directory);
    } catch (e) {
      setError(String(e));
    } finally {
      setExporting(false);
    }
  }

  const reserves = plan.segments.map(analyzeReserve);
  const blockingCount = reserves.filter((r) => r.status === "blocking").length;
  const marginalCount = reserves.filter((r) => r.status === "marginal").length;

  const narrationChars = plan.segments.reduce(
    (total, segment) => total + [...segment.narration].length,
    0,
  );
  const estimatedSecs = Math.max(1, narrationChars / 4);
  const narrationRatio =
    prepare.duration_secs > 0 ? (estimatedSecs / prepare.duration_secs) * 100 : 0;

  return (
    <section className="review-panel">
      <div className="eyebrow">DIRECTOR REVIEW</div>
      <div className="panel-intro review-heading">
        <div>
          <h2>导演审稿台</h2>
          <p>每段都应推动因果；保留原声只用于真正值得听见的台词。</p>
        </div>
        <span className="review-status">可编辑</span>
      </div>

      <StatusStepper status={status} progress={progress} kind={kind} />

      <VideoPreview sourcePath={prepare.source_path} outputPath={outputPath} />
      <TranscriptViewer taskId={prepare.task_id} source={prepare.subtitle_source} />
      <OpeningSamplePanel key={prepare.task_id} taskId={prepare.task_id} disabled={rendering || saving} />

      <label>
        成片标题
        <input
          type="text"
          value={plan.title}
          onChange={(e) => setPlan({ ...plan, title: e.target.value })}
        />
      </label>

      <div className="script-metrics">
        <div>
          <span>导演片段</span>
          <strong>{plan.segments.length}</strong>
        </div>
        <div>
          <span>解说字数</span>
          <strong>{narrationChars}</strong>
        </div>
        <div>
          <span>预计口播</span>
          <strong>{estimatedSecs.toFixed(0)}s</strong>
        </div>
        <div>
          <span>解说占原片</span>
          <strong>{narrationRatio.toFixed(0)}%</strong>
        </div>
        <div>
          <span>自动跳过</span>
          <strong>{prepare.excluded_ranges.length}</strong>
        </div>
        <div>
          <span>画面储备</span>
          <strong
            className={
              blockingCount > 0
                ? "metric-danger"
                : marginalCount > 0
                  ? "metric-warn"
                  : undefined
            }
          >
            {blockingCount > 0
              ? `${blockingCount} 段必补`
              : marginalCount > 0
                ? `${marginalCount} 段偏紧`
                : "充足"}
          </strong>
        </div>
      </div>

      {blockingCount > 0 && (
        <div className="reserve-banner blocking">
          有 {blockingCount} 段画面盖不住口播（标红段落）。可以让本地 AI 自动扩选画面并压缩仍然超时的旁白；渲染继续按原速播放，不用定格或慢放补时。
        </div>
      )}
      {blockingCount === 0 && marginalCount > 0 && (
        <div className="reserve-banner marginal">
          有 {marginalCount} 段画面余量不足口播的 1.2 倍（标黄段落）。实测配音偏慢时可能中止，建议扩选。
        </div>
      )}

      {prepare.excluded_ranges.length > 0 && (
        <div className="skip-chip-row review-skips">
          {prepare.excluded_ranges.map((range, index) => (
            <span className="skip-chip" key={`${range.kind}-${range.start}-${index}`}>
              {range.kind} {range.start.toFixed(0)}s–{range.end.toFixed(0)}s
            </span>
          ))}
        </div>
      )}

      <div className="segment-list">
        {plan.segments.map((seg, index) => {
          const reserve = reserves[index];
          return (
          <article key={`${index}-${seg.src_start}`} className="segment-card">
            <header>
              <div className="segment-index">
                <span>{String(index + 1).padStart(2, "0")}</span>
                <strong>导演片段</strong>
              </div>
              <span>
                {seg.src_start.toFixed(1)}s – {seg.src_end.toFixed(1)}s
              </span>
            </header>
            <div className="segment-times">
              <label>
                入点
                <input
                  type="number"
                  step="0.1"
                  value={seg.src_start}
                  onChange={(e) =>
                    updateSegment(index, {
                      src_start: Number(e.target.value),
                    })
                  }
                />
              </label>
              <label>
                出点
                <input
                  type="number"
                  step="0.1"
                  value={seg.src_end}
                  onChange={(e) =>
                    updateSegment(index, { src_end: Number(e.target.value) })
                  }
                />
              </label>
            </div>
            <textarea
              rows={4}
              value={seg.narration}
              onChange={(e) =>
                updateSegment(index, { narration: e.target.value })
              }
            />
            {!!seg.shots?.length && <div className="field-help">
              <p>本段连续配音，依次使用以下画面（原速）：</p>
              {seg.shots.map((shot, shotIndex) => <div key={shotIndex} className="segment-times">
                {[0, 1].map(side => <label key={side}>镜头 {shotIndex + 1} {side === 0 ? "入点" : "出点"}
                  <input type="number" step="0.1" value={shot[side]} onChange={e => {
                    const shots = seg.shots!.map(s => [...s] as [number, number]);
                    shots[shotIndex][side] = Number(e.target.value);
                    updateSegment(index, { shots });
                  }} />
                </label>)}
              </div>)}
            </div>}
            <label className="keep-original">
              <input
                type="checkbox"
                checked={Boolean(seg.keep_original_audio)}
                disabled={!!seg.shots?.length}
                onChange={(e) =>
                  updateSegment(index, {
                    keep_original_audio: e.target.checked,
                  })
                }
              />
              原声接力（先解说，再听原片对白；多镜头段不适用）
            </label>
            <div className="segment-footer">
              <span>
                {[...seg.narration].length} 字 · 口播约 {reserve.spoken.toFixed(1)}s · 画面{" "}
                {reserve.available.toFixed(1)}s
                {reserve.status === "handoff" ? " · 原声接力按对白校验" : ""}
              </span>
              <button
                type="button"
                className="text-btn danger"
                onClick={() => removeSegment(index)}
                disabled={plan.segments.length <= 1}
              >
                删除片段
              </button>
            </div>
            {(reserve.status === "blocking" || reserve.status === "marginal") && (
              <p className={`reserve-flag ${reserve.status}`}>
                {reserve.status === "blocking"
                  ? `画面不够：配音约 ${reserve.spoken.toFixed(1)}s，画面只有 ${reserve.available.toFixed(1)}s。扩选画面，或删约 ${reserve.cutChars} 字，否则生成成片会在这段中止。`
                  : `余量不足：画面 ${reserve.available.toFixed(1)}s，建议至少 ${reserve.reserve.toFixed(1)}s（口播的 1.2 倍），实测配音偏慢时可能中止。`}
              </p>
            )}
          </article>
          );
        })}
      </div>

      <button type="button" className="add-segment" onClick={addSegment}>
        ＋ 添加导演片段
      </button>

      {error && <p className="online-task-error">{error}</p>}
      {exportNotice && <p className="export-notice">{exportNotice}</p>}

      <div className="button-row">
        <button
          type="button"
          onClick={() => {
            if (
              window.confirm(
                "将按“平台字幕 → 视频内封字幕 → 本地画面硬字幕 OCR”的顺序重新获取字幕，并覆盖当前字幕和导演稿；不会使用语音识别。是否继续？",
              )
            ) {
              onRefetchSubtitle(prepare.task_id);
            }
          }}
          disabled={saving || rendering || status === "running"}
        >
          重新获取最佳字幕
        </button>
        <button
          type="button"
          onClick={() => onRegenerate(prepare.task_id)}
          disabled={saving || rendering || status === "running"}
        >
          {blockingCount > 0
            ? `AI 自动适配 ${blockingCount} 段`
            : "不满意，重新生成"}
        </button>
        <button type="button" onClick={handleSave} disabled={saving || rendering}>
          {saving ? "保存中..." : "保存改稿"}
        </button>
        <button
          type="button"
          onClick={handleExport}
          disabled={saving || exporting || rendering || status === "running"}
        >
          {exporting ? "正在导出..." : "导出解说文稿"}
        </button>
        <button
          type="button"
          className="primary"
          onClick={handleRender}
          disabled={rendering || blockingCount > 0}
          title={
            blockingCount > 0
              ? "有段落画面盖不住口播，先扩选画面或删字"
              : undefined
          }
        >
          {rendering
            ? "成片生成中..."
            : blockingCount > 0
              ? `先修 ${blockingCount} 段画面`
              : "生成成片"}
        </button>
        <button type="button" onClick={() => revealPath(prepare.job_dir)}>
          打开任务目录
        </button>
      </div>

    </section>
  );
}
