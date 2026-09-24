import { invoke } from "@tauri-apps/api/core";
export const previewEdgeVoice = (voice: string) => invoke<string>("preview_edge_voice", { voice });
import type { OpeningSample, SamplePlan } from "./types";

export const loadOpeningSample = (taskId: string) =>
  invoke<OpeningSample | null>("load_opening_sample", { taskId });
export interface SampleReviews {
  path?: string;
  reviews: { artifact_revision: string; report: { findings: { paragraph_index: number; verdict: string; reason: string; evidence_ids: number[] }[] } }[];
}
export const readSampleReviews = (taskId: string) => invoke<SampleReviews>("read_sample_reviews", { taskId });
export const generateOpeningSample = (taskId: string) =>
  invoke<OpeningSample>("generate_opening_sample", { taskId });
export const renderOpeningSample = (taskId: string, plan: SamplePlan, expectedRevision: string) =>
  invoke<OpeningSample>("render_opening_sample", { taskId, plan, expectedRevision });
import type {
  AppConfig,
  CommentaryExportResult,
  DependencyStatus,
  EditPlan,
  ExistingJob,
  OllamaStatus,
  PrepareResult,
  RenderResult,
} from "./types";

export async function prepareJob(
  url: string,
  taskId: string,
  style?: string,
): Promise<PrepareResult> {
  return invoke("prepare_job", {
    url,
    taskId,
    style,
  });
}

export async function renderJob(taskId: string): Promise<RenderResult> {
  return invoke("render_job", { taskId });
}

export async function saveEditPlan(
  taskId: string,
  plan: EditPlan,
): Promise<EditPlan> {
  return invoke("save_edit_plan", { taskId, plan });
}

export async function exportCommentaryScript(
  taskId: string,
): Promise<CommentaryExportResult> {
  return invoke("export_commentary_script", { taskId });
}

export async function loadJob(taskId: string): Promise<PrepareResult> {
  return invoke("load_job", { taskId });
}

export async function listExistingJobs(): Promise<ExistingJob[]> {
  return invoke("list_existing_jobs");
}

export async function readJobTranscript(taskId: string): Promise<string> {
  return invoke("read_job_transcript", { taskId });
}

export async function refetchPlatformSubtitleAndGenerate(
  taskId: string,
): Promise<PrepareResult> {
  return invoke("refetch_platform_subtitle_and_generate", { taskId });
}

export async function deleteJob(taskId: string): Promise<void> {
  return invoke("delete_job", { taskId });
}

export async function revealPath(path: string): Promise<void> {
  return invoke("reveal_path", { path });
}

export async function getSettings(): Promise<AppConfig> {
  return invoke("get_settings");
}

export async function saveSettings(config: AppConfig): Promise<void> {
  return invoke("save_settings", { config });
}

export async function checkOllama(baseUrl: string): Promise<OllamaStatus> {
  return invoke("check_ollama", { baseUrl });
}

export async function generateDirectorPlan(taskId: string): Promise<PrepareResult> {
  return invoke("generate_director_plan", { taskId });
}

export async function checkDependencies(): Promise<DependencyStatus> {
  return invoke("check_dependencies");
}
