export interface AppConfig {
  output_dir: string;
  cookies_browser: string;
  video_quality: string;
  tts_voice: string;
  original_audio_mode: string;
  duck_db: number;
  source_caption_mask_percent: number;
  background_music_path: string;
  background_music_db: number;
  default_style: string;
  ollama_base_url: string;
  ollama_model: string;
  skip_intro_outro: boolean;
  skip_ads: boolean;
  asr_fallback: boolean;
  asr_model_path: string;
  asr_language: string;
}

export interface AiStatus {
  available: boolean;
  model: string;
  error?: string | null;
}

export interface ExcludedRange {
  start: number;
  end: number;
  kind: string;
}

export interface PrepareResult {
  task_id: string;
  job_dir: string;
  source_path: string;
  transcript_path: string;
  source_available: boolean;
  transcript_available: boolean;
  subtitle_source: string;
  title: string;
  duration_secs: number;
  excluded_ranges: ExcludedRange[];
}

export interface ExistingJob {
  task_id: string;
  title: string;
  status: "running" | "ready" | "completed" | "failed";
  prepare: PrepareResult;
  error?: string | null;
  updated_at_ms: number;
  stage: string;
  progress: number;
  message: string;
  error_code?: string | null;
  retry_url?: string | null;
  shorts_total: number;
  shorts_rendered: number;
}

export interface TaskProgress {
  task_id?: string | null;
  step: string;
  percent: number;
  message: string;
}

export interface DependencyStatus {
  ytdlp: boolean;
  ffmpeg: boolean;
  ffmpeg_burn: boolean;
  local_tts: boolean;
  whisper?: boolean;
  whisper_model?: string | null;
  ytdlp_version: string | null;
  ffmpeg_version: string | null;
  ffmpeg_path: string | null;
}

export interface LocalVoice {
  name: string;
  language: string;
}

export const MAX_QUEUE_SIZE = 20;

export const STYLE_OPTIONS = [
  "剧情解说",
  "吐槽解说",
  "短剧解说",
  "科普解说",
  "纪录片旁白",
  "热点快评",
] as const;

export const STYLE_HINTS: Record<(typeof STYLE_OPTIONS)[number], string> = {
  吐槽解说: "损友旁观，先甩离谱再补上下文",
  剧情解说: "从人物的反常选择切入，讲清一段好戏",
  短剧解说: "强处境开场，打脸和悬念交替",
  科普解说: "先问矛盾，再用例子讲机制",
  纪录片旁白: "克制观察，细节先于评价",
  热点快评: "先结论，再用原片当证据",
};

export interface ShortScript {
  title: string;
  paragraphs: { narration: string; evidence_quote: string; shot_ids: number[] }[];
}

export interface SampleShot {
  id: number;
  start: number;
  end: number;
  text: string;
}

export interface ShortTopic {
  title: string;
  hook: string;
  angle: string;
  premise: string;
  score: number;
  start: number;
  end: number;
}

export interface ShortItem {
  id: string;
  topic: ShortTopic;
  revision: string;
  plan: ShortScript | null;
  shots: SampleShot[];
  error: string | null;
  output_path: string | null;
  duration_secs: number | null;
  warnings: string[];
}

export interface Episode {
  schema_version: number;
  transcript_revision: string;
  source_revision: string;
  revision: string;
  story: {
    spine: string;
    characters: string[];
    reveal_order: string;
    beats: { task: string; shot_ids: number[] }[];
    covered_events: { event_index: number; decision: string; reason: string }[];
  };
  plan: ShortScript;
  shots: SampleShot[];
  target_secs: number;
  reviewed_revision: string | null;
  review_error: string | null;
  research?: {
    query: { query: string; reason: string; paragraph_indices: number[] };
    sources: { title: string; url: string; snippet: string }[];
    error: string | null;
  }[];
  warnings: string[];
  output_path: string | null;
  duration_secs: number | null;
}

export interface ShortsBundle {
  transcript_revision: string;
  source_revision: string;
  items: ShortItem[];
}
