# inbox

这是应用唯一的任务数据根目录。下载、字幕、AI 导演、审稿、配音、剪辑和成片都保存在对应任务目录，不再使用仓库根目录的 `jobs/`。

`tasks.sqlite` 保存任务状态、阶段尝试、错误和产物版本；每个任务的 `job.json` 是方便人工诊断的状态快照。

桌面应用会调用设置中的 AI 模型自动生成导演稿。失败时从 `director/runs/` 的最后一个有效检查点恢复；已有成功稿时点击“不满意，重新生成”会开启全新一轮创作。

- `tasks.sqlite` — 状态与产物索引
- `{task_id}/source.mp4` — 原视频
- `{task_id}/TRANSCRIPT.srt` — 原字幕
- `{task_id}/director/` — 剧情节拍、蓝图、章节稿与事实裁决
- `{task_id}/review/` — 导演稿版本
- `{task_id}/tts/`、`clips/`、`renders/` — 内容寻址缓存与成片版本
- `{task_id}/edit.json`、`commentary.md`、`output.mp4` — 当前审稿与成片入口

详见仓库根目录 [`AGENTS.md`](../AGENTS.md)。
