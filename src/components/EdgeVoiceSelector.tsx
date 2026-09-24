import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { previewEdgeVoice } from "../lib/api";

// Same voice choices as the reference application's Edge selector; IDs remain editable.
const VOICES = [
  ["zh-CN-XiaoxiaoNeural", "晓晓 · 女声（默认）"],
  ["zh-CN-XiaoyiNeural", "晓伊 · 女声"],
  ["zh-CN-YunxiNeural", "云希 · 男声"],
  ["zh-CN-YunyangNeural", "云扬 · 男声"],
  ["zh-CN-YunjianNeural", "云健 · 男声"],
  ["zh-CN-XiaochenNeural", "晓辰 · 女声"],
] as const;

export function EdgeVoiceSelector({ voice, onChange }: { voice: string; onChange: (voice: string) => void }) {
  const [custom, setCustom] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [audio, setAudio] = useState<string | null>(null);
  const request = useRef(0);
  useEffect(() => {
    request.current += 1; setAudio(null); setError(""); setBusy(false);
    return () => { request.current += 1; };
  }, [voice]);
  const isCustom = custom || !VOICES.some(([id]) => id === voice);
  async function preview() {
    const id = ++request.current;
    setBusy(true); setError(""); setAudio(null);
    try {
      const path = await previewEdgeVoice(voice);
      if (request.current === id) setAudio(path);
    } catch (e) {
      if (request.current === id) setError(String(e));
    } finally {
      if (request.current === id) setBusy(false);
    }
  }
  return <div>
    <label>Edge TTS 音色
      <select value={isCustom ? "custom" : voice} onChange={e => {
        setCustom(e.target.value === "custom");
        if (e.target.value !== "custom") onChange(e.target.value);
      }}>
        {VOICES.map(([id,label]) => <option key={id} value={id}>{label}</option>)}
        <option value="custom">其他 / 自定义音色</option>
      </select>
    </label>
    {isCustom && <label>音色名称
      <input type="text" value={voice} placeholder="例如 zh-CN-XiaoxiaoNeural" onChange={e => onChange(e.target.value)} />
    </label>}
    <p className="field-help">{voice || "请输入音色名称"}。预设参考映序工作室，实际可用性以试听为准。</p>
    <button type="button" onClick={() => void preview()} disabled={busy || !voice.trim()}>{busy ? "正在生成试听…" : "试听所选音色"}</button>
    {audio && <audio key={audio} src={convertFileSrc(audio)} controls preload="metadata" aria-label="Edge TTS 音色试听" style={{width:"100%",marginTop:8}}
      onError={e => {
        const code = e.currentTarget.error?.code;
        setError(`试听音频已生成，但播放器无法读取或解码（错误 ${code ?? "未知"}）。请确认已重启到新版应用后重试试听。`);
      }} />}
    {audio && !error && <p className="field-help">试听已生成，请点击上方播放器的播放按钮。</p>}
    {error && <p role="alert">{error}</p>}
    <p className="field-help">试听需要联网，不必先保存。点击“保存设置”后用于后续样片和整片配音，不会改动已有视频。试听失败不会回退系统声音。</p>
  </div>;
}
