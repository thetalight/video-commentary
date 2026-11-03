import { useMemo } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { revealPath } from "../lib/api";

export function VideoPreview({ sourcePath }: { sourcePath: string }) {
  const src = useMemo(() => convertFileSrc(sourcePath), [sourcePath]);
  return (
    <section className="video-preview-panel">
      <div className="video-preview-head">
        <div>
          <span className="video-preview-kicker">SOURCE</span>
          <h3>原视频预览</h3>
        </div>
      </div>
      <video key={src} src={src} controls preload="metadata" playsInline className="preview-video" />
      <div className="video-preview-foot">
        <span title={sourcePath}>下载原片</span>
        <button type="button" className="text-btn" onClick={() => revealPath(sourcePath)}>
          在访达中显示
        </button>
      </div>
    </section>
  );
}
