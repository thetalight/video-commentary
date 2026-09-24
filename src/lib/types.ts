export interface AppConfig {
  output_dir: string;
  cookies_browser: string;
  video_quality: string;
  tts_voice: string;
  original_audio_mode: string;
  duck_db: number;
  burn_captions: boolean;
  source_caption_mask_percent: number;
  subtitle_font_size: number;
  background_music_path: string;
  background_music_db: number;
  default_target_duration_secs: number;
  default_style: string;
  ollama_base_url: string;
  ollama_model: string;
  ollama_vision_model: string;
  skip_intro_outro: boolean;
  skip_ads: boolean;
}

export interface OllamaStatus {
  available: boolean;
  base_url: string;
  models: string[];
  error?: string | null;
}

export interface ExcludedRange {
  start: number;
  end: number;
  kind: string;
}

export interface EditSegment {
  shots?: [number, number][];
  src_start: number;
  src_end: number;
  narration: string;
  keep_original_audio?: boolean;
}

export interface EditPlan {
  title: string;
  style: string;
  target_duration_secs: number;
  segments: EditSegment[];
}

export interface PrepareResult {
  task_id: string;
  job_dir: string;
  inbox_dir: string;
  source_path: string;
  transcript_path: string;
  source_available: boolean;
  transcript_available: boolean;
  subtitle_source: string;
  title: string;
  duration_secs: number;
  plan?: EditPlan | null;
  commentary: string;
  waiting_for_director: boolean;
  excluded_ranges: ExcludedRange[];
  director_error?: string | null;
}

export interface RenderResult {
  task_id: string;
  output_path: string;
  title: string;
}

export interface CommentaryExportResult {
  directory: string;
  text_path: string;
  srt_path?: string | null;
}

export interface ExistingJob {
  task_id: string;
  title: string;
  status: "queued" | "running" | "awaiting_director" | "ready" | "completed" | "failed";
  prepare: PrepareResult;
  output_path?: string | null;
  error?: string | null;
  updated_at_ms: number;
  stage: string;
  progress: number;
  message: string;
  error_code?: string | null;
  retry_url?: string | null;
}

export interface TaskProgress {
  task_id?: string | null;
  step: string;
  percent: number;
  message: string;
}

export interface TaskCompletePayload {
  task_id?: string | null;
  output_path: string;
}

export interface DependencyStatus {
  ytdlp: boolean;
  ffmpeg: boolean;
  ffmpeg_burn: boolean;
  hard_subtitle_ocr: boolean;
  edge_tts: boolean;
  ytdlp_version: string | null;
  ffmpeg_version: string | null;
  ffmpeg_path: string | null;
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
  剧情解说: "克制讲故事，沿人物选择推进并回扣主题",
  短剧解说: "强处境开场，打脸和悬念交替",
  科普解说: "先问矛盾，再用例子讲机制",
  纪录片旁白: "克制观察，细节先于评价",
  热点快评: "先结论，再用原片当证据",
};
export interface SamplePlan {
  title: string;
  paragraphs: { narration: string; evidence_quote: string; shot_ids: number[] }[];
}

export interface OpeningSample {
  revision: string;
  transcript_revision: string;
  source_revision: string;
  plan: SamplePlan;
  shots: { id: number; start: number; end: number; text: string }[];
  output_path: string | null;
  duration_secs: number | null;
  warnings: string[];
}
