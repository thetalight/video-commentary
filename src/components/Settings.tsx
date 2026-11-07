import { useEffect, useState } from "react";
import {
  checkDependencies,
  checkAi,
  getSettings,
  qwenTtsReady,
  listLocalVoices,
  saveSettings,
} from "../lib/api";
import type { AiStatus, AppConfig, DependencyStatus, LocalVoice } from "../lib/types";
import { STYLE_OPTIONS } from "../lib/types";
import { LocalVoiceSelector } from "./LocalVoiceSelector";

export function Settings() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [deps, setDeps] = useState<DependencyStatus | null>(null);
  const [qwenReady, setQwenReady] = useState(false);
  const [voices, setVoices] = useState<LocalVoice[]>([]);
  const [saved, setSaved] = useState(false);
  const [open, setOpen] = useState(false);
  const [ai, setAi] = useState<AiStatus | null>(null);
  const [checkingAi, setCheckingAi] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  useEffect(() => {
    getSettings().then(setConfig);
    checkDependencies().then(setDeps);
    listLocalVoices().then(setVoices);
    qwenTtsReady().then(setQwenReady);
  }, []);

  if (!config) return null;

  const hasMissingDeps =
    deps &&
    (!deps.ytdlp ||
      !deps.ffmpeg ||
      !deps.ffmpeg_burn ||
      !deps.local_tts);

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

  async function handleCheckAi() {
    setCheckingAi(true);
    try {
      setAi(await checkAi());
    } catch {
      setAi({ available: false, model: "", error: "无法检测 AI 配置，请重试" });
    } finally {
      setCheckingAi(false);
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
                {!deps!.local_tts && (
                  <li>
                    本机没有可用的 macOS 中文音色；请在系统设置中安装中文语音。
                  </li>
                )}
              </ul>
            </div>
          )}

          {deps?.ytdlp && deps.ffmpeg && (
            <div className="deps-ok">
              yt-dlp、ffmpeg 已就绪
              {deps.local_tts ? "，本机中文配音已就绪" : "；缺少本机中文音色，无法出片"}
            </div>
          )}

          <div className={`ai-status ${ai?.available ? "online" : "offline"}`}>
              <div>
                <strong>DeepSeek API</strong>
                <p>
                  {ai
                    ? ai.available
                      ? `已连接 · ${ai.model}`
                      : ai.error ?? "未连接"
                    : "通过后端 .env 配置；点击检测模型访问权限"}
                </p>
              </div>
              <button type="button" onClick={handleCheckAi} disabled={checkingAi}>
                {checkingAi ? "检测中..." : "检测连接"}
              </button>
          </div>
          <div className="warning">
            AI 梳理、写稿和评审会将原语言字幕及文案文本发送到 .env 配置的 API；
            自定义地址时，请确认该服务可信。配置TAVILY_API_KEY后，AI修稿可向Tavily发送少量关键词（人名/片名等可能外发），结果摘要再发送给DeepSeek；每轮工作流最多3次搜索，不发送整份字幕给搜索服务。原片画面、音轨不作为 AI 输入，转写和配音仍在本机完成。
          </div>

          <div className="settings-grid">
            <label>AI 运行方式<input value="DeepSeek API（后端 .env 配置）" disabled /></label>

            <p role="status">Qwen3-TTS 1.7B CustomVoice / MLX：{qwenReady ? "隔离环境及完整权重已就绪，请试听确认" : "尚未就绪，请运行安装脚本或等待完整权重下载；不会回退say"}</p>
            <LocalVoiceSelector voice={config.tts_voice} instruction={config.tts_instruction} voices={voices} onChange={voice => setConfig({ ...config, tts_voice: voice })} />

            <label className="span-2">Qwen朗读风格指令（不作为正文朗读）
              <textarea rows={3} maxLength={500} value={config.tts_instruction ?? ""} onChange={e=>setConfig({...config,tts_instruction:e.target.value})}/>
              <small>仅qwen3音色使用；例如“沉稳叙述，略带悬念，不喊叫”。改变指令会产生新的配音缓存；say不支持情绪指令。默认新任务音色为qwen3:Uncle_Fu，已有设置需手动选择并保存。</small>
            </label>
            <label className="span-2">本地背景音乐（可选，留空不添加）
              <input
                type="text"
                value={config.background_music_path}
                placeholder="粘贴本机音乐文件的完整路径"
                onChange={(event) =>
                  setConfig({ ...config, background_music_path: event.target.value })
                }
              />
              <small>仅读取本机文件；音乐循环铺底，解说出现时自动压低。</small>
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

            <label className="span-2">
              AI 配置说明
              <small>
                在后端 .env 设置 DEEPSEEK_API_KEY、DEEPSEEK_BASE_URL、DEEPSEEK_MODEL。
                默认模型ID为 deepseek-flash（官方接口已验证），模型名原样发送；API Key 不会传到此界面或保存在任务中。
                可选TAVILY_API_KEY用于AI修稿时联网查证；留空则只据字幕修稿。搜索结果仅供辅助，不是剧情证据，Key同样不进入前端或任务。
                修改文件后下次请求生效，进程环境变量优先；检测连接只读取DeepSeek模型列表，不检测搜索服务、不运行生成。
              </small>
            </label>

            <label>
              原片声音
              <select
                value={config.original_audio_mode}
                onChange={(e) =>
                  setConfig({ ...config, original_audio_mode: e.target.value })
                }
              >
                <option value="duck">压低铺底（默认，解说时再自动避让）</option>
                <option value="mute">静音（只留解说）</option>
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
              一帆正片最短时长
              <select
                value={config.yfsp_min_duration_secs}
                onChange={(e) =>
                  setConfig({ ...config, yfsp_min_duration_secs: Number(e.target.value) })
                }
              >
                <option value={0}>关闭时长检查</option>
                <option value={300}>5 分钟</option>
                <option value={600}>10 分钟（默认）</option>
                <option value={1200}>20 分钟</option>
                <option value={1800}>30 分钟</option>
              </select>
              <small>用于拦截误下的预览片；短剧可调低。设置对后续下载和任务恢复生效。</small>
            </label>

            <label className="span-2">
              裁掉原片底部（隐藏原片硬字幕）
              <select value={config.source_caption_mask_percent ?? 18} onChange={e =>
                setConfig({ ...config, source_caption_mask_percent: Number(e.target.value) })
              }>
                <option value={0}>不裁（原片没有硬字幕）</option>
                {[10,15,18,20,25,30,40].map(n => <option key={n} value={n}>底部 {n}%{n === 18 ? "（默认）" : ""}</option>)}
              </select>
              <small>横屏合成时裁掉原画面底部这一条，再等比缩放并补边到16:9，避免硬字幕与解说字幕叠加。若原片没有硬字幕请选择不裁，原视频文件不变。</small>
            </label>

            <label>
              排除明确片头 / 片尾署名
              <select
                value={config.skip_intro_outro ? "yes" : "no"}
                onChange={(e) =>
                  setConfig({ ...config, skip_intro_outro: e.target.value === "yes" })
                }
              >
                <option value="yes">开启（仅明确署名的实际字幕区间）</option>
                <option value="no">关闭（保留候选供核对）</option>
              </select>
              <small>无可靠标记不固定裁掉开头或结尾；音乐、重复台词、完结提示不自动排除。更改会使选片材料变化的旧稿失效，需重新生成。</small>
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

            <label>
              没有字幕时用本机语音转写
              <select
                value={config.asr_fallback ? "yes" : "no"}
                onChange={(e) =>
                  setConfig({ ...config, asr_fallback: e.target.value === "yes" })
                }
              >
                <option value="yes">开启（默认）</option>
                <option value="no">关闭（没有字幕就放弃）</option>
              </select>
              <small>
                平台独立字幕和视频内封字幕都没有时，用本机 whisper 从音轨生成带时间戳的逐字稿，全程不联网。关闭后这类视频会直接放弃。
              </small>
            </label>

            <label className="span-2">
              字幕获取策略
              <input value="平台字幕 → 视频内封字幕 → 本机语音转写（可关闭）" disabled />
              <small>不做画面 OCR，不全文翻译字幕；保留原语言与时间戳，DeepSeek 直接理解原文并写中文解说，证据引文保留原语言。</small>
            </label>

            <label className="span-2">
              任务目录
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
