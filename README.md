# 视频解说

## 项目文档

[Reflection 全系统设计与实施进度](/Users/product/video_commentary/docs/REFLECTION_SYSTEM_DESIGN.md)：Rig 接入、有界反思与整片分批终审的设计和实施记录。

[整体架构与流程（当前源码实现）](/Users/product/video_commentary/docs/ARCHITECTURE_AND_WORKFLOW.md)：涵盖模块职责、字幕获取、AI 导演、字数门槛、检查点、审稿、配音成片、任务重试与删除，并区分已实现行为和待改进事项。

[Story Pipeline v11](/Users/product/video_commentary/docs/STORY_PIPELINE_V9.md)：导演依次建立全片人物/关系模型、剧情取舍计划和 Narrative Unit；Writer 只交付旁白，程序再按已核验 beat 映射镜头，并在写作前锁定叙事预算。

[AI 导演流程深度审计与 v11 整改](/Users/product/video_commentary/docs/DIRECTOR_PIPELINE_AUDIT_2026-09-23.md)：记录 423.8 秒失败任务从初稿、事实核验到终审逐层缩水的证据、根因与本轮结构性改造。

基于 **Tauri 2 + React + Rust** 的桌面应用：粘贴视频链接，下载视频、抽取字幕；所有 AI 阶段仅调用本机 Ollama，再用 TTS + ffmpeg 重剪成片。字幕、导演稿与抽帧不发送到云模型。下载走 **yt-dlp**，凡它能解析的站点都可以（YouTube、Bilibili、X 等）。

成片以 **TTS 配音为主声**，原片按脚本裁切拼接，原声默认静音。经典名场面可在脚本里保留原声。成片时长由字幕里的关键节拍决定，不用手动填目标秒数。

解说量根据过滤后的有效字幕覆盖时长和剧情节拍分配；无字幕空镜、音乐、片头片尾和广告不产生机械字数债务。片段数量不设上限。

不再用“本章还差几字”触发循环补写。有效字幕覆盖时长的 30% 发布要求会在写作前转换为 Unit 级制作预算，并预留事实核验余量；Writer 一次交付完整旁白，事实核验只纠错不主动压成摘要，整片终审也不得把已达标稿件压回下限以下。预估口播时长不等于实际配音或成片时长，具体口径见整体流程文档。

## 流程

1. 粘贴链接，下载视频（yt-dlp）
2. 获取字幕（平台字幕优先，缺失则按设置使用 SenseVoice 或 Whisper）
3. 本机 Ollama 依次完成局部事实节拍、全片人物/关系模型、Story Plan、完整意群写作与独立复核；自动避开片头、片尾和广告
4. 页面自动进入审稿，可改词
5. edge-tts 配音，ffmpeg 按配音时长对齐画面并拼接

页面状态：**下载 → 字幕 → AI 导演 → 审稿 → 成片**。

应用启动时会扫描 `inbox/` 与持久化任务状态并恢复既有任务；审稿台可以直接切换播放原视频与已生成的成片。旧版 `jobs/{task_id}` 会在首次启动时无损合并进同名 `inbox/{task_id}`，冲突文件保存在任务的 `legacy/` 目录。

审稿台支持折叠查看任务的原始 `TRANSCRIPT.srt`，也可把当前改稿导出到任务的 `exports/解说文稿.txt`；已有与当前稿匹配的配音时间轴时会一并导出 `exports/解说字幕.srt`。任务列表中的“删除”会在确认后永久删除该任务唯一的 `inbox/{task_id}` 目录，包括原片、字幕、导演检查点、配音片段、导出文稿和成片。

### 开头样片实验（新增）

样片已使用 Rig 接入本机 Ollama，支持逐段评审与最多两轮局部修复；缺证据或修改退步会停止。页面可展开查看评审，草稿和修复记录保存在任务的 `samples/reflection/`。整片另有分批全稿评审与带证据局部修复。

审稿台的“先做 60–90 秒开头样片”独立于整片：先点“生成开头样片稿”，查看字幕依据、编辑口语文案和多镜头选片，再点“按此稿生成样片”并直接播放。可以单独重写样片，不覆盖整片稿或已有视频。

样片使用设置中的 AI，基于开头约 20 分钟的有效字幕提炼矛盾、创作意群、复核事实；不沿用旧稿。配音按段落生成，画面按实测配音长度使用多个原速片段，画面不足则提示补选，不用定格或大幅变速。60–90 秒仅作建议，偏离只提示，不触发凑字循环。

样片数据全部位于任务的 `samples/` 中，包括草稿、每次试制独立目录、短句字幕和视频。整片配音使用同次 TTS 请求返回的词级边界生成字幕；可选本机背景音乐，默认留空不添加。

目录：

```
inbox/
  tasks.sqlite            # 任务状态、阶段运行记录与产物版本
  {task_id}/              # 单一任务根目录
  META.json
  job.json                # 便于诊断的任务状态快照
  STYLE.md
  TRANSCRIPT.srt
  TRANSCRIPT.normalized.json
  TRANSCRIPT.compact.txt
  SCHEMA.md
  source.mp4              # 写稿阶段仍只读取字幕
  director/runs/          # 剧情节拍、Story Model、Story Plan、分章稿和事实裁决
  edit.json               # 片段数不限；可在审稿台“不满意，重新生成”
  commentary.md
  tts/                    # 按内容哈希缓存
  clips/                  # 按内容哈希缓存
  render-manifest.json    # 成片所用导演稿和设置版本
  output.srt
  output.mp4
```

## 环境要求

### 成片只显示解说字幕

默认在输出视频底部覆盖 18% 的黑色字幕区，再烧录解说字幕；样片和整片共用此处理，原片和字幕原文不修改。设置中可调整“原画面字幕遮盖高度”，没有硬字幕可设为 0。此方式是遮盖，不是无损去字或自动识别字幕位置；区域外的原字幕需调整高度。

启用遮盖时强制保留解说字幕，字幕烧录失败会报错，不再发布未处理字幕的临时成片。已有视频需重新生成成片才会应用；字幕与导演稿可复用。源文件中的独立字幕轨不复制到烧录后的输出。

### Edge TTS 音色选择

设置中的 Edge TTS 音色可选择晓晓、晓伊、云希、云扬、云健、晓辰，也可切换“其他 / 自定义音色”填写名称。选项参考 `/Users/demo/video` 的音色选择设计；是内置预设而非在线可用音色清单，实际可用性以服务响应为准。

点击“试听所选音色”由应用生成短句试听，无需先保存设置。试听和成片只使用 Edge TTS，失败不会切换系统声音；缓存位于 `inbox/.voice-previews/`。保存设置后用于后续样片和整片配音，旧视频保持不变。

```bash
# macOS
brew install yt-dlp ffmpeg-full
# 应用自动检测可运行的版本；烧录字幕需要 libass，无需强制 link

# 无字幕视频需要 Whisper
python3 -m venv ~/whisper-env
~/whisper-env/bin/pip install openai-whisper

# 中文配音
pip install edge-tts

# 可选：本地 AI 导演
ollama serve
ollama pull qwen3:8b
```

AI 固定使用本机 Ollama，可在设置中选择本地模型与回环地址。新版设置结构已删除云模型字段；首次读取旧设置时会清除旧供应商和密钥字段，运行时也拒绝任何非 Ollama 请求。

首次创建任务时，字幕会经过质量检测；有平台字幕时优先直接拉取，缺少平台字幕时按设置使用 SenseVoice 或 Whisper。审稿台点击“重新生成”时，只读取已经保存的 `TRANSCRIPT.srt` 并直接重跑 AI 导演，不重新下载或识别字幕。失败的导演任务会从最后一个有效检查点继续；已有成功导演稿时点击重新生成则开始一轮全新的创作。

## 开源参考

导演链路参考了 NarratoAI 的脚本校验思路、movie-narrator 的候选评审思路，以及 Yapper 的分阶段可恢复流水线。实际代码仅移植许可证兼容的部分，详见 `THIRD_PARTY_NOTICES.md`。

## 开发

```bash
npm install
npm run tauri dev
```

## 打包

```bash
npm run tauri build
```

成片固定在本仓库 `inbox/{task_id}/output.mp4`，所有任务资源都在同一个任务目录中。


## 测试链接 
 https://www.yfsp.tv/play/oFXGVHy4TzD
 https://www.yfsp.tv/play/zpfDhhNF3C9?id=bS77zt7A7WU
 https://www.yfsp.tv/play/bLrtiPqTmkB  
