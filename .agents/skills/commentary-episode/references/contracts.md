# 整集目标与当前短片实现的边界

所有路径相对于仓库根目录。修改程序前重新核对源码。

## 当前整集入口与基础契约

- 全集字幕分块梳理 → 合并证据索引与人物表 → 一份统一叙事方案 → 优先一次生成完整稿 → 逐段事实检查 + 全文连贯性检查 → 最多两轮局部修复 → 人工审稿 → 一条整集解说成片。
- 字幕读取块、写作调用块、事实评审批次均不是独立章节，更不是成片单位。中段不重复开场、介绍人物或独立收尾。
- 超出有效模型预算才连续续写，状态绑定源材料、方案和前文版本，包含人物称呼、已写内容、已揭示/未揭示信息、未回收铺垫、上段结尾、当前证据与已用画面 ID。
- 采用稳定全集源 ID 与完整稿段号；不能直接沿用每条短片重新编号的方式，否则跨块引文与镜头可能错位。
- 全文通过须程序校验、逐段事实评审、全文连贯性评审都绑定同一稿件版本且覆盖完整。不完整或不可用不是通过。
- 修复可联动接点两侧，但需明确允许段号、冻结其余内容；最多两轮，不能每块重开轮数。改主线或增删段须回到方案并请求确认，不隐式重写全文。
- 用户已确认原片约30%为初始预算、16:9横屏，不沿用一分钟窗口与稿件上限。
- commands/episode.rs 提供 load/generate/save/review/render_episode，均任务互斥；生成、保存、评审、出片校验当前版本。Episode容器在 services/episode.rs，字段包含源/字幕/正文revision、events、story、plan、shots、target_secs、reviewed_revision、review_error与输出。
- 新数据隔离在 episode/episode.json；runs/ 保存索引、方案、检查点与评审修复，renders/run-*/保存横屏视频和最终技术检查；不覆盖shorts。保存草稿可保留规则问题，修改使旧评审/输出引用失效。出片必须当前版本程序与双层评审通过并人工确认。
- 全集索引每块最多48窗、约6000字符，明确ID清单；缺失ID每批≤16窗、最多两轮补齐。先保存原始结果和覆盖诊断，不放行非法ID或补后仍不完整的索引。完整块绑定源/输入/模型/提示指纹保存在episode/runs/index-cache，重试可复用；不是正文自动续跑。
- 当前单次安全输入100000字符，长阶段输出24000 tokens，目标≤5000字符且输入可容纳时一次全文生成，否则按同一方案携带完整前文连续续写。超安全输入停止；没有自动层级索引或自动续跑，检查点仅供诊断。
- 新整集在最终混音和烧字幕后检查静音/黑场/静止及±0.5秒时钟差；输出1920×1080等比补边，不是旧竖屏。

## 可选联网与自动修稿

TAVILY_API_KEY由同一.env后端读取，留空只按字幕修稿。生成/单独审稿首次需修复时，AI可规划一次最多3条查询（各≤160字符、合法问题段）；只发关键词，过滤长原文摘录，不传完整字幕/稿件，固定Tavily搜索端点，不自动抓页或执行网页。结果摘要和来源存web-research.json/Episode.research，以untrusted参考传修稿与复审，不替代原文证据。搜索失败不视为通过。

整集insufficient_evidence进入修复：查相邻字幕、纠正/收窄/删除推断，最多两轮且重核全文，不因问题数增加提前失败；保留程序合格较好候选但不冒充终稿。通过当前版本才写accepted.json、允许人工确认后出片。以下旧shorts附录的立即停止规则不代表episode当前行为。

## AI 服务

`ai.rs / llm.rs` 已改为运行时读取后端 `.env` 的 `DEEPSEEK_API_KEY / DEEPSEEK_BASE_URL / DEEPSEEK_MODEL`；默认模型ID `deepseek-flash` 原样发送（官方已实测，原 DeepSeek-V4.1-Flash 字符串被官方拒绝），不自动回退。原语言字幕梳理、写稿、评审会外发到配置的服务；不先全文译中，证据引文逐字保留原语言，媒体输入、whisper、say 仍本机。DeepSeek 使用 JSON 对象模式，schema 以系统提示提供，现有 schema 的类型、必填字段、封闭字段、数组上限、枚举和数值范围由后端再次校验，反序列化与业务规则也由程序检查，不宣称服务端强制支持 JSON Schema。旧 Ollama 设置仅保留数据兼容。

## 附录：下文仅记录旧 shorts 代码（不代表整集入口）

下列字段、阈值、命令与路径用于诊断旧任务，不是整集目标的约束。不得通过把长稿塞入旧 `ShortTopic/ShortsBundle`、调大几项常量或只更换提示词，声称完成整集迁移。

## 1. 生产事实与文件归属

| 数据 | 生产者 / 消费者 | 约束 |
| --- | --- | --- |
| `TRANSCRIPT.srt`、`TRANSCRIPT.source.json` | `commands/pipeline.rs` | 应用规范字幕及来源，当前保留源语言不先全文译中；历史已安装译文不自动回滚；助手不改写 |
| `Shot { id, start, end, text }` | `script::film_shots` | 这是**字幕锚定时间窗**，不是视觉检测出的镜头，不能据此声称看到动作或表情 |
| `ShortTopic` | `shorts::scout` | `title, hook, angle, premise, score, start, end`；起止秒数由程序从合法 ID 映射，不由模型自由造时间 |
| `Script` | `shorts::write`、人工编辑 | `title, paragraphs[]`；段落字段仅 `narration, evidence_quote, shot_ids` |
| `ShortsBundle` | `commands/shorts.rs` | `transcript_revision, source_revision, items[]`，由应用写 `shorts/shorts.json` |
| `ShortItem` | 同上 | `id, topic, revision, plan, shots, error, output_path, duration_secs, warnings` |

任务 `META.json` 中的创建时风格优先于当前默认风格。`shorts.rs::style_guide` 只抽取 `styles/STYLES.md` 的同名节，未知风格回退“剧情解说”。

## 2. 选题与时间窗

源码：`script.rs::film_shots`、`shorts.rs::chunks / topics_from_report / expand_window / select`。

- 排除片头片尾、广告等区间；字幕相邻间隔 ≤2 秒可合并，窗口 ≤16 秒；生成后丢弃 <4 秒的窗，再连续编号。
- 每约 6000 字为一块，模型每块返回 0–3 个看点；`start_shot_id / end_shot_id` 必须在当前块中且不倒序。
- `angle` 枚举：名场面、反转、冲突、人物、金句、悬念；`score` 1–5。
- 程序向两侧扩展，目标累计字幕画面 ≥110 秒，原片跨度 ≤300 秒；不足 60 秒有效字幕画面就丢弃。**累计画面时长不等于 end-start，空档不能算储备。**
- 条数上限为 `round(片长秒数 / 600)`，再限制在 2–8；这是上限，不保证至少两条。
- 按分数优先选择；已有入选后不再选 <3 分。与已选窗口的交集大于两者较短窗口的 25% 就跳过，最后按原片时间排序。时间去重并不保证语义不重复。
- scout 用全片 ID；每条写稿只取窗口内镜头，**从 0 重新编号**，不得混用两种 ID。

模型 scout 返回形状（不是 `ShortTopic` 成品）：

```json
{"highlights":[{"start_shot_id":0,"end_shot_id":12,"title":"看点标题","hook":"有字幕依据的具体问题","angle":"冲突","premise":"必要处境与核心变化","score":4}]}
```

## 3. 写稿约束分级

### 写作目标，不是当前硬门禁

- 总字数 230–300，3–6 段，每段 30–70 字。
- 前 20 字内具体钩子；标题 6–14 字；结尾落在结果、反转、金句或真实悬念。
- 估计约一分钟，不能保证固定语速或精确 60 秒。

### `script::validate` 硬门禁

- 至少一段；每段非空文案且有合法镜头 ID。
- 镜头起止有效、无重复或时间重叠，**每段内**按时间排序；当前程序不要求跨段全局递增，叙事倒叙仍需人工核因果。
- `evidence_quote` 在所选镜头文本拼接后匹配：去非字母数字字符、转小写后至少 4 字符。创作仍应逐字引用连续原句，不利用跨窗拼接凑引文。
- 每段所选画面累计秒数 ≥ `narration.chars().count() × 0.3`；Rust 字符计数包含标点，不等于汉字数。
- 程序能验证引文存在，**不能证明整段所有事实成立**，也不证明画面语义匹配。

### 收尾 / 渲染警告

- 标题清理后 4–16 字可保留，否则回退选题标题。
- 总字数不在 180–340 内给警告，不是自动拒绝。
- 实测总配音不在 45–75 秒给警告，不是自动改速或截短。

稿件形状：

```json
{"title":"看点标题","paragraphs":[{"narration":"一段完整的口语叙事","evidence_quote":"所选字幕的原句","shot_ids":[0,1,2]}]}
```

示例只说明结构，不是可渲染样片；具体内容、ID、字数与储备仍需校验。

## 4. 评审与局部修复

源码：`reflection.rs::targets / apply / refine_with`、`shorts.rs::write`。

评审每段恰好一条，段号从 0 起：

```json
{"findings":[{"paragraph_index":0,"verdict":"revise","reason":"指出具体事实或表达问题及对应证据","evidence_ids":[0,1]}]}
```

- `verdict` 仅 `pass / revise / insufficient_evidence`。pass 必须带非空合法证据，不能漏段、重复段号或写空理由。
- 无法证实用 `insufficient_evidence` 并停止，不让模型猜；已有字幕足以改正才用 `revise`。
- 初审 + 最多两轮修复，最多三次评审。问题增多、修复无变化、过期版本或越权替换都会停止。
- 修复形状：`{"base_revision":"应用传入的原值","replacements":[{"paragraph_index":0,"paragraph":{"narration":"替换文本","evidence_quote":"原句","shot_ids":[0,1]}}]}`。只替换允许的问题段，不增删段、不改标题、不动其他段。
- `reflection` 失败后，`shorts::write` 可能保留 `best-candidate.json`，但仅限再次通过程序校验，并附 **AI 未完全通过** 警告。不能称为 accepted 或自动发布批准。

## 5. 真实记录路径

```text
shorts/
  shorts.json
  runs/
    run-<stamp>/
      scout/scout-001.json      # 各块原始模型结果
      scout/candidates.json
      scout/selected.json
      s01/
        evidence.json
        candidate-0.json       # 后续可能有 -1、-2
        review-0.json          # {artifact_revision, report}
        repair-0.json          # 后续可能有 -1
        best-candidate.json
        accepted.json          # 仅完全通过时存在
    rewrite-<stamp>/s01/        # 整条重写的同类记录
  tts/                         # 应用管理的内容/音色缓存
  s01/run-<stamp>/
    short.mp4
    short.srt
    short.ass
    technical-review.json
```

不能仅凭旧 `accepted.json` 判当前稿已通过：核对稿件版本与所属 run。人工修改后当前渲染入口只重新做程序校验，不自动再次运行 AI 语义评审。

## 6. 出片契约

源码：`commands/shorts.rs::render_short`、`shorts.rs::render`、`script.rs::timing`、`tts.rs`、`technical_review.rs`。

- 应用检查字幕版本、原片版本与 `expected_revision`，同一任务互斥；并发下载 2 / AI 2 / 渲染 1 由应用控制，skill 不另起并发池。
- 每段完整配音，取实测时长。画面可用时长不足、或比例分配后任一截取 <1 秒就失败；不截配音、不定格、不慢放。
- `timing` 缩短的是原片截取区间，不是改变播放速度；目前每窗从窗起点截取，不能承诺关键反应一定被保留。
- 画面 30fps；配音响度归一 -16 LUFS，原声压低并侧链避让；可选本地 BGM。
- 技术检查针对中间横屏 montage：应有配音处静音阻断，黑场/≥3 秒静止画面只提示。它不是对最终竖屏字幕、画面遮挡与混音乐音的完整验收。
- 竖屏 1080×1920，模糊暗背景 + 居中原画面；顶部黄标题，画面下方白色解说字幕；字幕每条约 ≤14 字，时间按实测配音时长估算，不是词级强制对齐。
- 最终时长与总配音差超过 ±0.5 秒阻断。新渲染成功后才保存 bundle 并清理同条旧渲染；失败不应把诊断目录里的文件当作最新交付。
- 人工稿的持久化随成功渲染完成，当前无独立保存命令；刷新前注意未保存编辑。
