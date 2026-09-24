import { useEffect, useState } from "react";
import {
  checkDependencies,
  checkOllama,
  getSettings,
  saveSettings,
} from "../lib/api";
import type { AppConfig, DependencyStatus, OllamaStatus } from "../lib/types";
import { STYLE_OPTIONS } from "../lib/types";
import { EdgeVoiceSelector } from "./EdgeVoiceSelector";

export function Settings() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [deps, setDeps] = useState<DependencyStatus | null>(null);
  const [saved, setSaved] = useState(false);
  const [open, setOpen] = useState(false);
  const [ollama, setOllama] = useState<OllamaStatus | null>(null);
  const [checkingOllama, setCheckingOllama] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  useEffect(() => {
    getSettings().then(setConfig);
    checkDependencies().then(setDeps);
  }, []);

  if (!config) return null;

  const hasMissingDeps =
    deps &&
    (!deps.ytdlp ||
      !deps.ffmpeg ||
      !deps.ffmpeg_burn ||
      !deps.edge_tts ||
      !deps.hard_subtitle_ocr);

  async function handleSave() {
    if (!config) return;
    setSaveError(null);
    try {
      await saveSettings(config);
      setConfig(await getSettings());
      setDeps(await checkDependencies());
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (error) {
      setSaveError(`保存设置失败：${String(error)}`);
    }
  }

  async function handleCheckOllama() {
    if (!config) return;
    setCheckingOllama(true);
    try {
      setOllama(await checkOllama(config.ollama_base_url));
    } finally {
      setCheckingOllama(false);
    }
  }

  return (
    <section className="settings-panel">
      <button
        type="button"
        className="settings-toggle"
        onClick={() => setOpen(!open)}
        aria-expanded={open}
      >
        <span>设置</span>
        <span className={`chevron ${open ? "open" : ""}`}>›</span>
      </button>

      {open && (
        <div className="settings-body">
          {hasMissingDeps && (
            <div className="warning">
              <strong>缺少依赖</strong>
              <ul>
                {!deps!.ytdlp && (
                  <li>
                    <code>brew install yt-dlp</code>
                  </li>
                )}
                {!deps!.ffmpeg && (
                  <li>
                    未找到可运行的 FFmpeg；若已安装，请检查依赖库是否损坏。
                    未安装时可使用 <code>brew install ffmpeg-full</code>。
                  </li>
                )}
                {deps!.ffmpeg && !deps!.ffmpeg_burn && (
                  <li>
                    FFmpeg 可运行，但未找到支持字幕烧录的版本。若已安装 ffmpeg-full，
                    请更新或重装以修复依赖；应用会自动检测，无需强制覆盖系统链接。
                  </li>
                )}
                {!deps!.hard_subtitle_ocr && (
                  <li>未找到本地画面字幕 OCR 组件，请重新构建或安装完整应用。</li>
                )}
                {!deps!.edge_tts && (
                  <li>
                    配音：<code>pip install edge-tts</code>
                  </li>
                )}
              </ul>
            </div>
          )}

          {deps?.ytdlp && deps.ffmpeg && (
            <div className="deps-ok">
              yt-dlp、ffmpeg 已就绪
              {deps.hard_subtitle_ocr ? "，本地画面字幕 OCR 已就绪" : "，画面字幕 OCR 未检测到"}
              {deps.edge_tts ? "，edge-tts 已就绪" : "，将尝试系统 say 兜底"}
            </div>
          )}

          <div className={`ollama-status ${ollama?.available ? "online" : "offline"}`}>
              <div>
                <strong>本地 Ollama</strong>
                <p>
                  {ollama
                    ? ollama.available
                      ? `已连接 · ${ollama.models.length} 个模型`
                      : ollama.error ?? "未连接"
                    : "点击检测本地导演服务"}
                </p>
              </div>
              <button type="button" onClick={handleCheckOllama} disabled={checkingOllama}>
                {checkingOllama ? "检测中..." : "检测连接"}
              </button>
          </div>

          <div className="settings-grid">
            <label>AI 运行方式<input value="本机 Ollama（不上传字幕、脚本或画面）" disabled /></label>

            <EdgeVoiceSelector voice={config.tts_voice} onChange={voice => setConfig({ ...config, tts_voice: voice })} />

            <label className="span-2">本地背景音乐（可选，留空不添加）
              <input
                type="text"
                value={config.background_music_path}
                placeholder="粘贴本机音乐文件的完整路径"
                onChange={(event) =>
                  setConfig({ ...config, background_music_path: event.target.value })
                }
              />
              <small>仅读取本机文件；音乐会循环铺底，并在解说或原声出现时自动压低。</small>
            </label>
            <label>背景音乐音量
              <input
                type="number"
                min="-40"
                max="-8"
                step="1"
                value={config.background_music_db}
                onChange={(event) =>
                  setConfig({ ...config, background_music_db: Number(event.target.value) })
                }
              />
            </label>

            <label>
              默认风格
              <select
                value={config.default_style}
                onChange={(e) =>
                  setConfig({ ...config, default_style: e.target.value })
                }
              >
                {STYLE_OPTIONS.map((s) => (
                  <option key={s} value={s}>
                    {s}
                  </option>
                ))}
              </select>
            </label>

              <>
                <label>
                  Ollama 模型
                  <input
                    type="text"
                    list="ollama-models"
                    value={config.ollama_model}
                    placeholder="留空自动选择，推荐 qwen3:8b"
                    onChange={(e) =>
                      setConfig({ ...config, ollama_model: e.target.value })
                    }
                  />
                  <datalist id="ollama-models">
                    {ollama?.models.map((model) => (
                      <option key={model} value={model} />
                    ))}
                  </datalist>
                </label>

                <label>
                  Ollama API
                  <input
                    type="text"
                    value={config.ollama_base_url}
                    onChange={(e) =>
                      setConfig({ ...config, ollama_base_url: e.target.value })
                    }
                  />
                </label>

                <label>
                  粗剪视觉评审模型
                  <input
                    type="text"
                    list="ollama-models"
                    value={config.ollama_vision_model}
                    placeholder="留空自动选择本机视觉模型"
                    onChange={(event) =>
                      setConfig({ ...config, ollama_vision_model: event.target.value })
                    }
                  />
                  <small>仅把粗剪抽帧发送到本机 Ollama；不会使用云模型。</small>
                </label>
              </>

            <label>
              原声
              <select
                value={config.original_audio_mode}
                onChange={(e) =>
                  setConfig({ ...config, original_audio_mode: e.target.value })
                }
              >
                <option value="mute">静音</option>
                <option value="duck">压低（-20 dB）</option>
              </select>
            </label>

            <label>
              画质
              <select
                value={config.video_quality}
                onChange={(e) =>
                  setConfig({ ...config, video_quality: e.target.value })
                }
              >
                <option value="best">最佳</option>
                <option value="1080">1080p</option>
                <option value="720">720p</option>
              </select>
            </label>

            <label>
              浏览器 Cookies
              <select
                value={config.cookies_browser}
                onChange={(e) =>
                  setConfig({ ...config, cookies_browser: e.target.value })
                }
              >
                <option value="">不使用</option>
                <option value="chrome">Chrome</option>
                <option value="brave">Brave</option>
                <option value="edge">Edge</option>
              </select>
            </label>

            <label>
              原画面字幕遮盖高度
              <select value={config.source_caption_mask_percent ?? 18} onChange={e => {
                const percent = Number(e.target.value);
                setConfig({ ...config, source_caption_mask_percent: percent, burn_captions: percent > 0 || config.burn_captions });
              }}>
                <option value={0}>不遮盖（原片没有硬字幕）</option>
                {[10,15,18,20,25,30,40].map(n => <option key={n} value={n}>底部 {n}%{n === 18 ? "（默认）" : ""}</option>)}
              </select>
              <small>先用黑色字幕区遮住底部原字幕，再显示解说字幕。会遮挡对应画面；位置较高的原字幕需调大高度。原视频不变。</small>
            </label>

            <label>
              烧录解说字幕
              <select
                value={config.burn_captions || (config.source_caption_mask_percent ?? 18) > 0 ? "yes" : "no"}
                disabled={(config.source_caption_mask_percent ?? 18) > 0}
                onChange={(e) =>
                  setConfig({ ...config, burn_captions: e.target.value === "yes" })
                }
              >
                <option value="yes">开启</option>
                <option value="no">关闭</option>
              </select>
            </label>

            <label>
              跳过片头 / 片尾
              <select
                value={config.skip_intro_outro ? "yes" : "no"}
                onChange={(e) =>
                  setConfig({ ...config, skip_intro_outro: e.target.value === "yes" })
                }
              >
                <option value="yes">开启</option>
                <option value="no">关闭</option>
              </select>
            </label>

            <label>
              跳过中间广告
              <select
                value={config.skip_ads ? "yes" : "no"}
                onChange={(e) =>
                  setConfig({ ...config, skip_ads: e.target.value === "yes" })
                }
              >
                <option value="yes">开启</option>
                <option value="no">关闭</option>
              </select>
            </label>

            <label className="span-2">
              字幕获取策略
              <input value="平台字幕 → 视频内封字幕 → 本地画面硬字幕 OCR" disabled />
              <small>不使用语音识别；三种字幕都不可用时任务会停止。</small>
            </label>

            <label className="span-2">
              任务目录（固定在源码 inbox）
              <div className="dir-row">
                <input type="text" value={config.output_dir} readOnly />
              </div>
            </label>
          </div>

          <div className="settings-footer">
            {saveError && <p className="online-task-error">{saveError}</p>}
            <button type="button" className="primary" onClick={handleSave}>
              {saved ? "已保存" : "保存设置"}
            </button>
          </div>
        </div>
      )}
    </section>
  );
}
