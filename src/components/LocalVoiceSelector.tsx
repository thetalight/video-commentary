import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { previewLocalVoice } from "../lib/api";
import type { LocalVoice } from "../lib/types";

export function LocalVoiceSelector({ voice, voices, onChange }: {
  voice: string;
  voices: LocalVoice[];
  onChange: (voice: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [audio, setAudio] = useState<string | null>(null);
  const request = useRef(0);

  useEffect(() => {
    request.current += 1;
    setAudio(null);
    setError("");
    setBusy(false);
    return () => { request.current += 1; };
  }, [voice]);

  async function preview() {
    const id = ++request.current;
    setBusy(true);
    setError("");
    setAudio(null);
    try {
      const path = await previewLocalVoice(voice);
      if (request.current === id) setAudio(path);
    } catch (e) {
      if (request.current === id) setError(String(e));
    } finally {
      if (request.current === id) setBusy(false);
    }
  }

  return <div>
    <label>本机中文音色
      <select value={voice} onChange={e => onChange(e.target.value)} disabled={voices.length === 0}>
        {voices.length === 0 && <option value={voice}>未找到本机中文音色</option>}
        {voices.map(item => <option key={item.name} value={item.name}>
          {item.name} · {item.language}
        </option>)}
      </select>
    </label>
    <p className="field-help">配音由 macOS 本机合成，不调用语音云服务；AI 写稿仍会通过配置的 DeepSeek API 处理文本。字幕时间依据实测音频时长估算，可能有少量偏差。</p>
    <button type="button" onClick={() => void preview()} disabled={busy || voices.length === 0}>
      {busy ? "正在生成试听…" : "试听所选音色"}
    </button>
    {audio && <audio key={audio} src={convertFileSrc(audio)} controls preload="metadata" aria-label="本机音色试听" style={{ width: "100%", marginTop: 8 }}
      onError={() => setError("试听已生成，但播放器无法读取音频。请检查本机媒体支持。")}
    />}
    {error && <p role="alert">{error}</p>}
  </div>;
}
