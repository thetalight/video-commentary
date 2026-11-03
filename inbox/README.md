# inbox

开发时，应用把任务保存在仓库的 `inbox/`；独立安装且没有源码目录时，保存在当前用户的应用数据目录 `com.esun.video-commentary/inbox/`。设置页显示实际路径。

- `tasks.sqlite`：任务状态、阶段、错误和产物索引。
- `{task_id}/job.json`：方便诊断的任务状态快照。
- `{task_id}/META.json`、`source.mp4`、`TRANSCRIPT.srt`、`TRANSCRIPT.source.json`：原片与字幕依据。
- `{task_id}/shorts/shorts.json`：短片选题、文案、镜头、出片结果。
- `{task_id}/shorts/runs/`：每次选题、写稿和逐段评审的记录。
- `{task_id}/shorts/tts/`：本机配音与估算字幕时间缓存。
- `{task_id}/shorts/s01/run-*/short.mp4`：短片成片；每条只保留最近一次成功渲染。

诊断时可读取任务数据；不要直接修改 `META.json` 或字幕原文。
