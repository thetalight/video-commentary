import { useEffect, useMemo, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { revealPath } from "../lib/api";

interface VideoPreviewProps {
  sourcePath: string;
  outputPath?: string;
}

export function VideoPreview({ sourcePath, outputPath }: VideoPreviewProps) {
  const [active, setActive] = useState<"source" | "output">("source");
  const sourceSrc = useMemo(() => convertFileSrc(sourcePath), [sourcePath]);
  const outputSrc = useMemo(
    () => (outputPath ? convertFileSrc(outputPath) : null),
    [outputPath],
  );

  useEffect(() => {
    setActive((current) => {
      if (outputSrc) return "output";
      return current === "output" ? "source" : current;
    });
  }, [outputSrc]);

  const playingOutput = active === "output" && Boolean(outputSrc);
  const activePath = playingOutput ? outputPath! : sourcePath;
  const activeSrc = playingOutput ? outputSrc! : sourceSrc;

  return (
    <section className="video-preview-panel">
      <div className="video-preview-head">
        <div>
          <span className="video-preview-kicker">DESKTOP PREVIEW</span>
          <h3>{playingOutput ? "成片预览" : "原视频预览"}</h3>
        </div>
        <div className="video-preview-tabs" role="tablist" aria-label="视频预览切换">
          <button
            type="button"
            className={active === "source" ? "active" : ""}
            onClick={() => setActive("source")}
            role="tab"
            aria-selected={active === "source"}
          >
            原视频
          </button>
          <button
            type="button"
            className={active === "output" ? "active" : ""}
            onClick={() => setActive("output")}
            disabled={!outputSrc}
            role="tab"
            aria-selected={active === "output"}
          >
            {outputSrc ? "成片" : "成片待生成"}
          </button>
        </div>
      </div>

      <video
        key={activeSrc}
        src={activeSrc}
        controls
        preload="metadata"
        playsInline
        className="preview-video"
      />

      <div className="video-preview-foot">
        <span title={activePath}>{playingOutput ? "最终成片" : "下载原片"}</span>
        <button type="button" className="text-btn" onClick={() => revealPath(activePath)}>
          在访达中显示
        </button>
      </div>
    </section>
  );
}
