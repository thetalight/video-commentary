# Qwen3-TTS 1.7B CustomVoice / MLX

用户已明确批准该单一模型。本次安装隔离Python3.12环境，固定mlx-audio==0.6.4；下载mlx-community/Qwen3-TTS-12Hz-1.7B-CustomVoice-bf16（revision 52f4770fd9726457eae3d3b6aa92047a25a10776），没有Base/VoiceDesign/其他模型、没有声音克隆。

## 当前状态

- 路径：当前用户应用数据目录com.esun.video-commentary/qwen-tts/venv及models/Qwen3-TTS-12Hz-1.7B-CustomVoice-bf16。
- 主权重3,833,402,589字节 + speech tokenizer 682,293,092字节，共约4.52GB；另有运行依赖。两份权重SHA256核验完成，下载临时片段已清理；权重不提交到Git。
- MLX/Metal导入成功。强制HF_HUB_OFFLINE/TRANSFORMERS_OFFLINE，使用本地完整模型目录；生成不触发下载、不上传文字/音频、不回退say。
- 虚构短句、Uncle_Fu、克制悬念指令已实际合成5.84秒24kHz单声道PCM WAV，证明运行成功；未人工听审、未验证完整长稿稳定性/效果，不宣称无漏读或情感完全遵循。

## 使用

重启桌面应用，在设置中选qwen3:Uncle_Fu（默认新配置）、qwen3:Serena或qwen3:Vivian等，填“Qwen朗读风格指令”，试听比较后保存。已有设置若为Tingting不会静默改动，须手动选择Qwen。say仅在用户显式选择其音色时使用。

首次安装另一个机器可运行scripts/install-qwen-tts.sh（需要uv）。脚本仅安装同一批准模型及依赖，不生成任务产物。当前安装遇过网络慢/超时，下载须完整后才可用。

风格指令与正文分离，不读指令；全片默认保持同音色，指令建议克制，避免句句强情绪。缓存包含提供方、实际权重文件身份、音色、文本、指令、固定采样实现版本；新权重/指令不命中旧say缓存。旧音频仍保留，不修改任务原字幕/META。

单段最多1200字符、生成token预算2048，触顶视为可能截断，拒绝发布；单次子进程15分钟上限。当前每段加载模型，有冷启动成本，持续服务后端尚未实现；大量短段效率需实际测量。所有音频仍独立检查可解码性/静音、按实测时长估算字幕；未引入强制对齐模型，能量检查不证明全文读对。

## 许可

Qwen原模型Apache-2.0：https://huggingface.co/Qwen/Qwen3-TTS-12Hz-1.7B-CustomVoice
MLX运行实现为第三方项目，MIT：https://github.com/Blaizzy/mlx-audio
模型转换库：https://huggingface.co/mlx-community/Qwen3-TTS-12Hz-1.7B-CustomVoice-bf16
再分发权重/依赖时须携带对应许可证并重新核查版本；本仓库只加入独立适配器和安装脚本，不捆绑模型。
