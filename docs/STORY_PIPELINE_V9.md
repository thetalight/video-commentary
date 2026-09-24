# Story Pipeline v11：先理解全片，再写解说

## 为什么重构

旧流程已经能够提取字幕事件、定位原片时间和生成可渲染片段，但章节仍按固定数量的局部剧情节拍分组。结果容易把“进入办公室、问候、解释招聘、发现误会、接受工作”分别写成五六句旁白：局部事实大多正确，整片却像字幕摘要，人物关系也可能在不同章节漂移。

v9 的核心合同是：**局部事实节拍不是解说段落。必须先建立全片 Story Model，再由 Story Planner 取舍并合并成 Narrative Unit，Writer 最后才写旁白。**

## 实际执行链

```text
TRANSCRIPT.srt
  → MAP：局部事实节拍（带稳定 B0001… 编号）
  → Story Builder：story-model.json
  → Story Planner：story-plan.json / Narrative Unit[]
  → Narration Budget：把发布下限前置为全片及 Unit 制作预算
  → Writer：只按 Narrative Unit 写旁白（chapter-*.script.json）
  → Timeline Mapper：程序按 beat_id 绑定证据镜头
  → Fact Reviewer：逐段字幕证据核验
  → Full Reviewer：人物、关系、重复、颗粒度和文风终审
  → Timeline Compiler：原速画面、原声交接和口播储备
  → TTS / 字幕 / 渲染
```

所有 AI 阶段继续使用设置中的本地 Ollama 模型。不同阶段使用不同角色、Schema、上下文和温度；不是多个自由聊天的 Agent，也不依赖云模型。

## 1. 局部事实 MAP

MAP 只回答“发生了什么”，不写文案。每个节拍记录时间、摘要、重要度、叙事层级和可选原声台词。程序过滤片头、片尾、广告与 framing 内容，按原片顺序分配稳定 `beat_id`，并从只读原字幕附上该时间附近的 `evidence`。Story Builder 因此能同时看到局部模型的摘要和原字幕；二者冲突时以原字幕为准，而不是让第一轮误判一路传下去。

操作步骤、寒暄、报价、重复对白或没有改变人物目标、关系、认识的信息不应成为高重要度节拍。此层允许保留较多候选，因为真正的取舍发生在 Planner。

## 2. 全片 Story Model

`story-model.json` 是全片一致性合同，包含：

- 主角欲望、核心冲突、代价、中心问题和结尾回应；
- `character_bible`：稳定人物 ID、自然称呼、明确别称、角色和带 `beat_id` 的人物事实；
- `relationships`：人物之间的关系变化及逐条证据；
- `story_threads`：建立、发展和回收的故事线；
- 人物弧线、关系弧线、因果链和反复出现的物件或处境。

程序会删除不存在的证据引用、重复人物 ID、无证据人物事实、无效关系和无效故事线。身份冲突必须保留 uncertainty；片名和电影常识不是证据。

## 3. Story Planner

`story-plan.json` 决定“这部电影到底怎么讲”。每个 Narrative Unit 合并服务同一戏剧目的的多个局部节拍，并记录：

- 进入状态 `entry_state`；
- 真正发生的转折 `turn`；
- 离开状态 `exit_state`；
- 本段旁白目标 `narration_goal`；
- 引用的 `beat_ids`；
- `narration / original_dialogue / transition / emotional_pause` 音频策略；
- 重要度和可选原声锚点。

Unit 数量只按片长给规划参考，不为数字拆段。程序还会执行四条确定性防线：

1. 无效、重复或越界的 `beat_id` 被移除；
2. importance 4–5 的核心转折即使被模型漏掉，也会并入时间上最接近的已有 Unit，而不是新增碎段；
3. `original_dialogue` 必须引用本 Unit 内确有完整 quote 的节拍，否则降级为普通旁白；
4. 如果模型退化成“一事件一 Unit”，相邻低层 Unit 会合并到按片长计算的上限内；
5. 如果 Planner 把同一个 Unit 放到时间线上多次、跨过其他 Unit 又折返，程序会拆成单向的连续 run。任何 Unit 都不能用一个几百秒的大时间窗包住中间剧情。

未被选择的局部事实写入 `omitted_beat_ids`，明确表示“知道，但决定不讲”，不再把覆盖所有细节误当成质量。

## 4. Writer

Writer 同时看到全片 Story Model、完整 Story Plan、当前 Narrative Units、上一章结尾、对应局部节拍和原字幕证据。人物表是身份合同，局部理解派生层只能解释歧义，不能覆盖全片人物关系。

v11 将写稿与剪辑决策彻底分开。Writer 只输出 `lines: {unit_id: narration}`，不再生成 `src_start`、`src_end` 或 `shots`。每个 Narrative Unit 恰好交付一个完整旁白段，段落骨架是“人物状态/处境 → 关键事件或选择 → 造成的变化”，禁止按字幕事件写成“发生 A、说了 B、回答 C、然后 D”。程序随后依据 Unit 已核验的 `beat_ids` 确定镜头；AI 文案不能自行圈选无关画面。

30% 发布要求在写作前转换为预算：目标初稿在有效字幕覆盖口径的发布下限上保留有限审核余量，但不超过证据口径允许的上限。预算按 Unit 的重要度和证据量分配，并写入结构化输出 Schema 的 `minLength`；因此它是第一次交付合同，不是终审结束后的累计补写任务。`narration-budget.json` 保存本次下限、初稿目标、上限和各 Unit 预算，方便审查。

## 5. Reviewer 与时间轴

章节事实 Reviewer 仍逐段验证字幕证据，但它是事实编辑而不是摘要器：修正错误时要保留同段已证明的叙事作用与信息密度。整片 Reviewer 只运行一轮，负责以下整片级硬问题：

- 姓名、亲属、职业和关系是否与 character_bible 冲突；
- 剧情顺序、称呼与 character_bible 是否前后一致；
- 相邻段是否实质重复；
- 是否残留 ASR 说话人编号；
- 修订后的旁白是否仍由本段证据支持。

终审不再以“还能更简洁”为理由重复改写。如果整轮修改会把旁白压到发布下限以下，程序拒绝整轮修改并保留章节事实核验后的版本。每章最后一段扩选画面时还会提前使用下一章首镜作为边界，禁止跨章抢占画面。

审核通过后才进入既有 Timeline Compiler。画面仍按原速播放，不靠慢放或定格补口播；当前渲染层对 `original_dialogue` 使用原声接力，`transition` 与 `emotional_pause` 暂时仍输出普通解说画面，不虚构对白或假装已实现独立环境声轨。

## 检查点与重试

当前实现使用 `director-v11-script-budget-linear-timeline` 作为提示版本。v11 保留 v10 的 Story Model、动态 Unit 区间和全片取舍，同时新增单向时间线、Unit 级旁白预算、独立 Script 层和确定性镜头映射。旧版本检查点不会复用。已有字幕仍直接从 AI 导演开始，不重新下载或识别。

任务目录中的关键产物：

```text
director/runs/{revision}/
  beats.json
  story-model.json
  story-plan.json
  narration-budget.json
  chapters/
    chapter-*.script.json
  full-review/
  final-plan.json
```

## 当前边界

- 导演阶段继续只使用字幕，因此人物表不能依赖未写入字幕的服装、表情或构图信息；粗剪视觉评审只负责本地成片抽检。
- 本地模型的 confidence 是自评值，不是统计校准概率；程序依赖证据编号和结构约束，而不是盲信分数。
- `original_ambient`、独立音乐段和真正的情绪留白尚未进入渲染数据模型。本版先解决全片人物一致性、剧情取舍和旁白颗粒度，不能把规划字段描述成已经完成的多轨声音设计。
