import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  DependencyStatus,
  ExistingJob,
  Episode,
  ExclusionReport,
  LocalVoice,
  AiStatus,
  PrepareResult,
  ShortScript,
  ShortsBundle,
} from "./types";

export const prepareJob = (url: string, taskId: string, style?: string) =>
  invoke<PrepareResult>("prepare_job", { url, taskId, style });
export const listExistingJobs = () => invoke<ExistingJob[]>("list_existing_jobs");
export const readJobTranscript = (taskId: string) =>
  invoke<string>("read_job_transcript", { taskId });
export const refetchSubtitles = (taskId: string) =>
  invoke<PrepareResult>("refetch_subtitles", { taskId });
export const deleteJob = (taskId: string) => invoke<void>("delete_job", { taskId });
export const revealPath = (path: string) => invoke<void>("reveal_path", { path });

export const loadShorts = (taskId: string) =>
  invoke<ShortsBundle | null>("load_shorts", { taskId });
export const generateShorts = (taskId: string) =>
  invoke<ShortsBundle>("generate_shorts", { taskId });
export const rewriteShort = (taskId: string, shortId: string) =>
  invoke<ShortsBundle>("rewrite_short", { taskId, shortId });
export const renderShort = (
  taskId: string,
  shortId: string,
  plan: ShortScript,
  expectedRevision: string,
) => invoke<ShortsBundle>("render_short", { taskId, shortId, plan, expectedRevision });

export const episodeExclusions = (taskId: string) => invoke<ExclusionReport>("episode_exclusions", { taskId });
export const loadEpisode = (taskId: string) => invoke<Episode | null>("load_episode", { taskId });
export const generateEpisode = (taskId: string, expectedRevision: string | null) =>
  invoke<Episode>("generate_episode", { taskId, expectedRevision });
export const saveEpisode = (taskId: string, plan: ShortScript, expectedRevision: string) =>
  invoke<Episode>("save_episode", { taskId, plan, expectedRevision });
export const reviewEpisode = (taskId: string, expectedRevision: string) =>
  invoke<Episode>("review_episode", { taskId, expectedRevision });
export const renderEpisode = (taskId: string, expectedRevision: string, confirmedReview: boolean) =>
  invoke<Episode>("render_episode", { taskId, expectedRevision, confirmedReview });

export const getSettings = () => invoke<AppConfig>("get_settings");
export const saveSettings = (config: AppConfig) => invoke<void>("save_settings", { config });
export const checkAi = () => invoke<AiStatus>("check_ai");
export const checkDependencies = () => invoke<DependencyStatus>("check_dependencies");
export const previewLocalVoice = (voice: string) =>
  invoke<string>("preview_local_voice", { voice });
export const listLocalVoices = () => invoke<LocalVoice[]>("list_local_voices");
