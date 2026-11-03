use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum FfmpegError {
    #[error(
        "未找到可运行的 FFmpeg / ffprobe；可能未安装或依赖库已损坏，请检查应用设置中的依赖状态"
    )]
    NotFound,
    #[error("ffmpeg failed: {0}")]
    Failed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

const RENDER_AUDIO_RATE: &str = "48000";
const RENDER_AUDIO_CHANNELS: &str = "2";

#[cfg(test)]
mod resolver_tests {
    use super::*;

    #[test]
    fn skips_existing_but_unusable_preferred_installation() {
        let broken = PathBuf::from("/opt/homebrew/opt/ffmpeg-full/bin/ffprobe");
        let working = PathBuf::from("/opt/homebrew/bin/ffprobe");
        let mut checked = Vec::new();
        let selected = first_usable(vec![broken.clone(), working.clone()], |path| {
            checked.push(path.to_path_buf());
            path == working
        });
        assert_eq!(selected, Some(working.clone()));
        assert_eq!(checked, vec![broken, working]);
    }

    #[test]
    fn does_not_assume_path_fallback_is_working() {
        assert_eq!(first_usable(vec![PathBuf::from("ffmpeg")], |_| false), None);
    }

    #[test]
    fn subtitle_capability_requires_an_actual_filter_entry() {
        assert!(lists_subtitles_filter(
            " ... subtitles V->V Render text subtitles onto input video using the libass library."
        ));
        assert!(!lists_subtitles_filter("Unknown filter 'subtitles'."));
        assert!(!lists_subtitles_filter(
            " ... ass V->V Render ASS subtitles."
        ));
    }

    #[test]
    fn searches_past_a_candidate_without_subtitle_support() {
        let basic = PathBuf::from("basic");
        let full = PathBuf::from("full");
        assert_eq!(
            first_usable(vec![basic, full.clone()], |path| path == full),
            Some(full)
        );
    }

    #[test]
    fn render_audio_contract_requires_48khz_stereo() {
        assert!(render_audio_contract_matches(
            r#"{"streams":[{"sample_rate":"48000","channels":2}]}"#
        ));
        assert!(!render_audio_contract_matches(
            r#"{"streams":[{"sample_rate":"44100","channels":2}]}"#
        ));
        assert!(!render_audio_contract_matches(
            r#"{"streams":[{"sample_rate":"48000","channels":1}]}"#
        ));
        assert!(!render_audio_contract_matches(r#"{"streams":[]}"#));
        assert!(!render_audio_contract_matches("not json"));
    }

    #[test]
    fn embedded_subtitles_prefer_chinese_and_ignore_bitmap_tracks() {
        let streams = parse_embedded_subtitle_streams(
            r#"{"streams":[
                {"index":4,"codec_name":"hdmv_pgs_subtitle","tags":{"language":"zh"}},
                {"index":3,"codec_name":"subrip","tags":{"language":"jpn"}},
                {"index":2,"codec_name":"mov_text","tags":{"language":"zh-Hans"}},
                {"index":5,"codec_name":"webvtt","tags":{"language":"eng"}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(streams.len(), 3);
        assert_eq!(streams[0].index, 2);
        assert_eq!(streams[1].index, 3);
        assert_eq!(streams[2].index, 5);
    }
}

fn ffmpeg_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(custom) = std::env::var("FFMPEG_PATH") {
        candidates.push(PathBuf::from(custom));
    }

    for path in [
        "/opt/homebrew/opt/ffmpeg-full/bin/ffmpeg",
        "/usr/local/opt/ffmpeg-full/bin/ffmpeg",
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
    ] {
        candidates.push(PathBuf::from(path));
    }

    candidates.push(PathBuf::from("ffmpeg"));
    candidates
}

fn has_subtitles_filter(ffmpeg: &Path) -> bool {
    Command::new(ffmpeg)
        .args(["-hide_banner", "-filters"])
        .output()
        .map(|o| o.status.success() && lists_subtitles_filter(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false)
}

fn lists_subtitles_filter(output: &str) -> bool {
    output
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some("subtitles"))
}

fn can_start(binary: &Path) -> bool {
    Command::new(binary)
        .arg("-version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn first_usable(
    candidates: Vec<PathBuf>,
    mut usable: impl FnMut(&Path) -> bool,
) -> Option<PathBuf> {
    candidates.into_iter().find(|path| usable(path))
}

pub fn resolve_ffmpeg_bin() -> Option<PathBuf> {
    first_usable(ffmpeg_candidates(), can_start)
}

fn ffprobe_candidates() -> Vec<PathBuf> {
    // Probe independently: the preferred FFmpeg's sibling may also be broken.
    ffmpeg_candidates()
        .into_iter()
        .map(|mut ffmpeg| {
            let name = ffmpeg
                .file_name()
                .map(|n| n.to_string_lossy().replace("ffmpeg", "ffprobe"))
                .unwrap_or_else(|| "ffprobe".to_string());
            ffmpeg.set_file_name(name);
            ffmpeg
        })
        .collect()
}

fn resolve_ffprobe() -> Option<PathBuf> {
    first_usable(ffprobe_candidates(), can_start)
}

pub fn resolve_ffmpeg() -> Option<PathBuf> {
    first_usable(ffmpeg_candidates(), has_subtitles_filter)
}

fn run_cmd(cmd: &mut Command) -> Result<(), FfmpegError> {
    let output = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FfmpegError::NotFound
        } else {
            FfmpegError::Io(e)
        }
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(FfmpegError::Failed(stderr.to_string()));
    }
    Ok(())
}

pub fn subtitles_filter_hint() -> String {
    "未找到可运行且支持字幕烧录的 FFmpeg（需要 subtitles / libass）。\n\
     若已安装 ffmpeg-full，可能是 Homebrew 升级后依赖库不匹配，请更新或重装 ffmpeg-full；\n\
     若尚未安装，请执行 brew install ffmpeg-full。应用会自动检测，无需强制覆盖系统链接。"
        .to_string()
}

pub fn get_duration(media_path: &Path) -> Result<f64, FfmpegError> {
    let ffprobe = resolve_ffprobe().ok_or(FfmpegError::NotFound)?;
    let output = Command::new(&ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            "-i",
            media_path.to_str().unwrap_or_default(),
        ])
        .output()
        .map_err(FfmpegError::Io)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(FfmpegError::Failed(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .trim()
        .parse::<f64>()
        .map_err(|e| FfmpegError::Failed(format!("Invalid duration: {e}")))
}

pub fn get_dimensions(video_path: &Path) -> Result<(u32, u32), FfmpegError> {
    let ffprobe = resolve_ffprobe().ok_or(FfmpegError::NotFound)?;
    let output = Command::new(&ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0:s=x",
            "-i",
            video_path.to_str().unwrap_or_default(),
        ])
        .output()
        .map_err(FfmpegError::Io)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(FfmpegError::Failed(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut parts = stdout.trim().split('x');
    let width: u32 = parts
        .next()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| FfmpegError::Failed("Missing video width".into()))?;
    let height: u32 = parts
        .next()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| FfmpegError::Failed("Missing video height".into()))?;
    Ok((width, height))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedSubtitleStream {
    pub index: u32,
    pub codec: String,
    pub language: String,
}

#[derive(Debug, Deserialize)]
struct ProbeSubtitleTags {
    #[serde(default)]
    language: String,
}

#[derive(Debug, Deserialize)]
struct ProbeSubtitleStream {
    index: u32,
    #[serde(default)]
    codec_name: String,
    #[serde(default)]
    tags: Option<ProbeSubtitleTags>,
}

#[derive(Debug, Deserialize)]
struct ProbeSubtitleOutput {
    #[serde(default)]
    streams: Vec<ProbeSubtitleStream>,
}

fn is_text_subtitle_codec(codec: &str) -> bool {
    matches!(
        codec.to_ascii_lowercase().as_str(),
        "subrip" | "srt" | "ass" | "ssa" | "webvtt" | "mov_text" | "text" | "ttml"
    )
}

fn subtitle_language_rank(language: &str) -> u8 {
    let language = language.to_ascii_lowercase();
    if language.starts_with("zh") || matches!(language.as_str(), "chi" | "zho") {
        0
    } else if language.starts_with("ja") || language == "jpn" {
        1
    } else if language.starts_with("en") || language == "eng" {
        2
    } else if language.starts_with("ko") || language == "kor" {
        3
    } else {
        4
    }
}

fn parse_embedded_subtitle_streams(
    value: &str,
) -> Result<Vec<EmbeddedSubtitleStream>, FfmpegError> {
    let output: ProbeSubtitleOutput = serde_json::from_str(value)
        .map_err(|error| FfmpegError::Failed(format!("无法解析内封字幕信息：{error}")))?;
    let mut streams = output
        .streams
        .into_iter()
        .filter(|stream| is_text_subtitle_codec(&stream.codec_name))
        .map(|stream| EmbeddedSubtitleStream {
            index: stream.index,
            codec: stream.codec_name,
            language: stream
                .tags
                .map(|tags| tags.language)
                .filter(|language| !language.trim().is_empty())
                .unwrap_or_else(|| "und".to_string()),
        })
        .collect::<Vec<_>>();
    streams.sort_by_key(|stream| subtitle_language_rank(&stream.language));
    Ok(streams)
}

pub fn embedded_subtitle_streams(
    media_path: &Path,
) -> Result<Vec<EmbeddedSubtitleStream>, FfmpegError> {
    let ffprobe = resolve_ffprobe().ok_or(FfmpegError::NotFound)?;
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "s",
            "-show_entries",
            "stream=index,codec_name:stream_tags=language",
            "-of",
            "json",
            "-i",
        ])
        .arg(media_path)
        .output()
        .map_err(FfmpegError::Io)?;
    if !output.status.success() {
        return Err(FfmpegError::Failed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    parse_embedded_subtitle_streams(&String::from_utf8_lossy(&output.stdout))
}

/// Extract the best embedded TEXT subtitle track. Bitmap tracks such as PGS
/// are deliberately excluded because transcoding them to SRT without OCR
/// would either fail or silently produce an empty file.
pub fn extract_best_embedded_subtitle(
    media_path: &Path,
    output_dir: &Path,
) -> Result<Option<PathBuf>, FfmpegError> {
    let Some(stream) = embedded_subtitle_streams(media_path)?.into_iter().next() else {
        return Ok(None);
    };
    fs::create_dir_all(output_dir)?;
    let language = stream
        .language
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect::<String>();
    let output_path = output_dir.join(format!(
        "source.embedded.{}.srt",
        if language.is_empty() {
            "und"
        } else {
            &language
        }
    ));
    let ffmpeg = resolve_ffmpeg_bin().ok_or(FfmpegError::NotFound)?;
    let output = Command::new(ffmpeg)
        .args(["-y", "-v", "error", "-i"])
        .arg(media_path)
        .args(["-map", &format!("0:{}", stream.index), "-c:s", "srt"])
        .arg(&output_path)
        .output()
        .map_err(FfmpegError::Io)?;
    if !output.status.success() {
        return Err(FfmpegError::Failed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    if output_path
        .metadata()
        .is_ok_and(|metadata| metadata.len() > 0)
    {
        Ok(Some(output_path))
    } else {
        Ok(None)
    }
}

pub fn has_audio(media_path: &Path) -> bool {
    let Some(ffprobe) = resolve_ffprobe() else {
        return false;
    };
    Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=index",
            "-of",
            "csv=p=0",
        ])
        .arg(media_path)
        .output()
        .is_ok_and(|output| output.status.success() && !output.stdout.trim_ascii().is_empty())
}

/// Every cached paragraph clip must satisfy one audio contract before concat.
/// The concat demuxer does not reject mixed AAC sample rates; it can instead
/// produce a successful file with long silent holes at a format boundary.
fn render_audio_contract_matches(output: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output) else {
        return false;
    };
    let Some(stream) = value.get("streams").and_then(|value| value.get(0)) else {
        return false;
    };
    stream.get("sample_rate").and_then(|value| value.as_str()) == Some(RENDER_AUDIO_RATE)
        && stream.get("channels").and_then(|value| value.as_u64())
            == RENDER_AUDIO_CHANNELS.parse::<u64>().ok()
}

pub fn has_render_audio_contract(media_path: &Path) -> bool {
    let Some(ffprobe) = resolve_ffprobe() else {
        return false;
    };
    let Ok(output) = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=sample_rate,channels",
            "-of",
            "json",
            "-i",
            media_path.to_str().unwrap_or_default(),
        ])
        .output()
    else {
        return false;
    };
    output.status.success()
        && render_audio_contract_matches(&String::from_utf8_lossy(&output.stdout))
}

/// A paragraph spans multiple native-speed pictures. In duck mode, their source
/// ambience is concatenated and side-chain compressed under narration.
pub fn narration_montage(
    source: &Path,
    ranges: &[(f64, f64)],
    audio: &Path,
    output: &Path,
    duration: f64,
    width: u32,
    height: u32,
    orig_volume: f64,
) -> Result<(), FfmpegError> {
    let ffmpeg = resolve_ffmpeg_bin().ok_or(FfmpegError::NotFound)?;
    if ranges.is_empty() {
        return Err(FfmpegError::Failed("段落没有画面".into()));
    }
    let mut args = vec!["-y".to_string()];
    let mut filter = String::new();
    for (i, (start, end)) in ranges.iter().enumerate() {
        args.extend([
            "-ss".into(),
            format!("{start:.6}"),
            "-t".into(),
            format!("{:.6}", end - start),
            "-i".into(),
            source.to_string_lossy().into_owned(),
        ]);
        filter.push_str(&format!("[{i}:v]setpts=PTS-STARTPTS,fps=30,scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,setsar=1,format=yuv420p[v{i}];"));
        if orig_volume > 0.001 {
            filter.push_str(&format!("[{i}:a]atrim=0:{:.6},asetpts=PTS-STARTPTS,aresample={RENDER_AUDIO_RATE}:async=0:first_pts=0,aformat=channel_layouts=stereo[o{i}];",end-start));
        }
    }
    args.extend(["-i".into(), audio.to_string_lossy().into_owned()]);
    if orig_volume > 0.001 {
        for i in 0..ranges.len() {
            filter.push_str(&format!("[v{i}][o{i}]"));
        }
        filter.push_str(&format!(
            "concat=n={}:v=1:a=1[v][orig];\
             [{}:a]asetpts=PTS-STARTPTS,loudnorm=I=-16:TP=-1.5:LRA=11,aresample={RENDER_AUDIO_RATE}:async=0:first_pts=0,aformat=channel_layouts=stereo,asplit=2[tts][side];\
             [orig]volume={orig_volume:.8}[amb];\
             [amb][side]sidechaincompress=threshold=0.025:ratio=8:attack=15:release=350[ducked];\
             [tts][ducked]amix=inputs=2:duration=first:dropout_transition=0:normalize=0,alimiter=limit=0.95,aresample={RENDER_AUDIO_RATE}:async=0:first_pts=0[a]",
            ranges.len(),
            ranges.len()
        ));
    } else {
        for i in 0..ranges.len() {
            filter.push_str(&format!("[v{i}]"));
        }
        filter.push_str(&format!("concat=n={}:v=1:a=0[v];[{}:a]asetpts=PTS-STARTPTS,loudnorm=I=-16:TP=-1.5:LRA=11,aresample={RENDER_AUDIO_RATE}:async=0:first_pts=0,aformat=channel_layouts=stereo[a]",ranges.len(),ranges.len()));
    }
    args.extend([
        "-filter_complex".into(),
        filter,
        "-map".into(),
        "[v]".into(),
        "-map".into(),
        "[a]".into(),
        "-t".into(),
        format!("{duration:.6}"),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "fast".into(),
        // Intermediate generation: the vertical compose re-encodes once more.
        "-crf".into(),
        "16".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        "-ar".into(),
        RENDER_AUDIO_RATE.into(),
        "-ac".into(),
        RENDER_AUDIO_CHANNELS.into(),
        "-movflags".into(),
        "+faststart".into(),
        output.to_string_lossy().into_owned(),
    ]);
    run_cmd(Command::new(ffmpeg).args(args))
}

pub fn concat_videos(inputs: &[PathBuf], output_path: &Path) -> Result<(), FfmpegError> {
    let ffmpeg = resolve_ffmpeg_bin().ok_or(FfmpegError::NotFound)?;
    if inputs.is_empty() {
        return Err(FfmpegError::Failed("没有可拼接的片段".into()));
    }
    if let Some((index, path)) = inputs
        .iter()
        .enumerate()
        .find(|(_, path)| !has_render_audio_contract(path))
    {
        return Err(FfmpegError::Failed(format!(
            "第 {} 个剪辑不符合统一的 48kHz 双声道音频格式，已在拼接前停止；请重新渲染该段，不能继续生成可能含静音洞的成片：{}",
            index + 1,
            path.display()
        )));
    }
    if inputs.len() == 1 {
        fs::copy(&inputs[0], output_path)?;
        return Ok(());
    }

    let list_path = output_path.with_extension("concat.txt");
    let mut list = String::new();
    for path in inputs {
        let escaped = path
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('\'', "'\\''");
        list.push_str(&format!("file '{escaped}'\n"));
    }
    fs::write(&list_path, list)?;

    // Every paragraph is encoded independently. Stream-copying their AAC tracks
    // can preserve encoder priming and discontinuous packet timestamps even when
    // the concat command exits successfully, producing long silent holes in the
    // rough cut. Keep the already encoded video, but rebuild one continuous audio
    // clock from decoded samples.
    let result = run_cmd(Command::new(&ffmpeg).args([
        "-y",
        "-fflags",
        "+genpts",
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
        list_path.to_str().unwrap_or_default(),
        "-c:v",
        "copy",
        "-af",
        "asetpts=N/SR/TB",
        "-c:a",
        "aac",
        "-b:a",
        "192k",
        "-ar",
        RENDER_AUDIO_RATE,
        "-ac",
        RENDER_AUDIO_CHANNELS,
        "-avoid_negative_ts",
        "make_zero",
        "-movflags",
        "+faststart",
        output_path.to_str().unwrap_or_default(),
    ]));

    if result.is_err() {
        run_cmd(Command::new(&ffmpeg).args([
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            list_path.to_str().unwrap_or_default(),
            "-vf",
            "setpts=N/(30*TB),fps=30",
            "-af",
            "asetpts=N/SR/TB",
            "-c:v",
            "libx264",
            "-preset",
            "fast",
            "-crf",
            "20",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-ar",
            RENDER_AUDIO_RATE,
            "-ac",
            RENDER_AUDIO_CHANNELS,
            "-avoid_negative_ts",
            "make_zero",
            "-movflags",
            "+faststart",
            output_path.to_str().unwrap_or_default(),
        ]))?;
    }

    let _ = fs::remove_file(&list_path);
    Ok(())
}

/// Loop a user-selected local music file under the completed dialogue track.
/// The dialogue is the side-chain signal, so music automatically retreats when
/// narration or retained original dialogue is audible.
pub fn mix_background_music(
    video_path: &Path,
    music_path: &Path,
    output_path: &Path,
    music_db: f64,
) -> Result<(), FfmpegError> {
    let ffmpeg = resolve_ffmpeg_bin().ok_or(FfmpegError::NotFound)?;
    if video_path == output_path {
        return Err(FfmpegError::Failed(
            "背景音乐混音的输入与输出不能是同一文件".into(),
        ));
    }
    if !music_path.is_file() {
        return Err(FfmpegError::Failed("本地背景音乐文件不存在".into()));
    }
    let duration = get_duration(video_path)?;
    if !duration.is_finite() || duration <= 0.0 {
        return Err(FfmpegError::Failed("无法读取视频时长".into()));
    }
    let gain = 10f64.powf(music_db / 20.0);
    let filter = format!(
        "[0:a]aresample={RENDER_AUDIO_RATE}:async=0:first_pts=0,aformat=channel_layouts=stereo,asplit=2[dialogue][side];\
         [1:a]aresample={RENDER_AUDIO_RATE}:async=0:first_pts=0,aformat=channel_layouts=stereo,volume={gain:.8},\
         atrim=0:{duration:.6},asetpts=PTS-STARTPTS[music];\
         [music][side]sidechaincompress=threshold=0.025:ratio=10:attack=15:release=450[ducked];\
         [dialogue][ducked]amix=inputs=2:duration=first:dropout_transition=0:normalize=0,\
         alimiter=limit=0.95,aresample={RENDER_AUDIO_RATE}:async=0:first_pts=0[a]"
    );
    run_cmd(Command::new(&ffmpeg).args([
        "-y",
        "-i",
        video_path.to_str().unwrap_or_default(),
        "-stream_loop",
        "-1",
        "-i",
        music_path.to_str().unwrap_or_default(),
        "-filter_complex",
        &filter,
        "-map",
        "0:v:0",
        "-map",
        "[a]",
        "-t",
        &format!("{duration:.6}"),
        "-c:v",
        "copy",
        "-c:a",
        "aac",
        "-b:a",
        "192k",
        "-ar",
        RENDER_AUDIO_RATE,
        "-ac",
        RENDER_AUDIO_CHANNELS,
        "-movflags",
        "+faststart",
        output_path.to_str().unwrap_or_default(),
    ]))
}

pub const VERTICAL_W: u32 = 1080;
pub const VERTICAL_H: u32 = 1920;
const LOCAL_ASS: &str = "short-captions.ass";

/// Height the source picture occupies inside the 9:16 canvas after the bottom
/// `mask_percent` (burned-in source subtitles) is cropped away.
pub fn vertical_foreground_height(src_w: u32, src_h: u32, mask_percent: u32) -> u32 {
    let kept = src_h as f64 * (1.0 - mask_percent.min(40) as f64 / 100.0);
    let scale = (VERTICAL_W as f64 / src_w.max(1) as f64).min(VERTICAL_H as f64 / kept.max(1.0));
    ((kept * scale / 2.0).round() as u32 * 2).min(VERTICAL_H)
}

fn vertical_filter(mask_percent: u32) -> String {
    let mask = mask_percent.min(40);
    let crop = if mask == 0 {
        String::new()
    } else {
        format!("crop=iw:trunc(ih*(100-{mask})/200)*2:0:0,")
    };
    format!(
        "[0:v]split=2[bgsrc][fgsrc];\
         [bgsrc]scale={VERTICAL_W}:{VERTICAL_H}:force_original_aspect_ratio=increase,crop={VERTICAL_W}:{VERTICAL_H},boxblur=24:2,eq=brightness=-0.22[bg];\
         [fgsrc]{crop}scale={VERTICAL_W}:{VERTICAL_H}:force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1[fg];\
         [bg][fg]overlay=(W-w)/2:(H-h)/2,subtitles=filename={LOCAL_ASS},format=yuv420p[v]"
    )
}

/// Recompose a landscape (or any) montage into a 9:16 short: blurred fill,
/// centered picture, and an ASS layer carrying the title and narration captions.
pub fn compose_vertical(
    video_path: &Path,
    ass_path: &Path,
    output_path: &Path,
    mask_percent: u32,
) -> Result<(), FfmpegError> {
    let ffmpeg = resolve_ffmpeg().ok_or_else(|| FfmpegError::Failed(subtitles_filter_hint()))?;
    if video_path == output_path {
        return Err(FfmpegError::Failed(
            "Input and output video paths must differ".into(),
        ));
    }
    let output_dir = output_path
        .parent()
        .ok_or_else(|| FfmpegError::Failed("Output path has no parent directory".into()))?;
    fs::create_dir_all(output_dir)?;
    let local_ass = output_dir.join(LOCAL_ASS);
    if fs::canonicalize(ass_path).ok() != fs::canonicalize(&local_ass).ok() {
        fs::copy(ass_path, &local_ass)?;
    }
    let video_input = fs::canonicalize(video_path).unwrap_or_else(|_| video_path.to_path_buf());
    let pending_output = output_path.with_extension("vertical-pending.mp4");
    let result = run_cmd(Command::new(&ffmpeg).current_dir(output_dir).args([
        "-y",
        "-i",
        video_input.to_str().unwrap_or_default(),
        "-filter_complex",
        &vertical_filter(mask_percent),
        "-map",
        "[v]",
        "-map",
        "0:a",
        "-c:v",
        "libx264",
        "-preset",
        "medium",
        "-crf",
        "18",
        "-profile:v",
        "high",
        "-c:a",
        "copy",
        "-movflags",
        "+faststart",
        pending_output.to_str().unwrap_or_default(),
    ]));
    if let Err(error) = result {
        let _ = fs::remove_file(&pending_output);
        return Err(error);
    }
    fs::rename(pending_output, output_path)?;
    Ok(())
}

fn landscape_filter(mask_percent: u32) -> String {
    let mask = mask_percent.min(40);
    format!("crop=iw:trunc(ih*(100-{mask})/200)*2:0:0,scale=1920:1080:force_original_aspect_ratio=decrease:force_divisible_by=2,pad=1920:1080:(ow-iw)/2:(oh-ih)/2,setsar=1,subtitles=filename={LOCAL_ASS},format=yuv420p")
}

/// Fixed 16:9 delivery. Crop the old subtitle strip, fit/pad without stretching.
pub fn compose_landscape(
    video_path: &Path,
    ass_path: &Path,
    output_path: &Path,
    mask_percent: u32,
) -> Result<(), FfmpegError> {
    let ffmpeg = resolve_ffmpeg().ok_or_else(|| FfmpegError::Failed(subtitles_filter_hint()))?;
    let output_dir = output_path
        .parent()
        .ok_or_else(|| FfmpegError::Failed("Output path has no parent".into()))?;
    fs::create_dir_all(output_dir)?;
    let local_ass = output_dir.join(LOCAL_ASS);
    fs::copy(ass_path, &local_ass)?;
    let input = fs::canonicalize(video_path)?;
    let output = output_dir.join("landscape-pending.mp4");
    let filter = landscape_filter(mask_percent);
    let result = run_cmd(Command::new(ffmpeg).current_dir(output_dir).args([
        "-y",
        "-i",
        input.to_str().unwrap_or_default(),
        "-vf",
        &filter,
        "-map",
        "0:v:0",
        "-map",
        "0:a:0",
        "-c:v",
        "libx264",
        "-preset",
        "medium",
        "-crf",
        "18",
        "-c:a",
        "copy",
        "-movflags",
        "+faststart",
        output.to_str().unwrap_or_default(),
    ]));
    if let Err(error) = result {
        let _ = fs::remove_file(output);
        return Err(error);
    }
    fs::rename(output, output_path)?;
    Ok(())
}

#[cfg(test)]
mod vertical_tests {
    use super::*;
    #[test]
    fn episode_output_fits_landscape_without_stretch_or_slow_motion() {
        let filter = landscape_filter(18);
        assert!(filter.contains("scale=1920:1080:force_original_aspect_ratio=decrease"));
        assert!(filter.contains("pad=1920:1080"));
        assert!(filter.contains("100-18"));
        assert!(landscape_filter(90).contains("100-40"));
        assert!(!filter.contains("boxblur"));
        assert!(!filter.contains("setpts"));
    }

    #[test]
    fn landscape_source_sits_in_the_middle_band() {
        assert_eq!(vertical_foreground_height(1920, 1080, 0), 608);
        assert!(vertical_foreground_height(1920, 1080, 18) < 608);
        assert_eq!(vertical_foreground_height(1080, 1920, 0), 1920);
    }
    #[test]
    fn mask_crops_instead_of_drawing_a_bar() {
        assert!(vertical_filter(18).contains("crop=iw:trunc(ih*(100-18)/200)*2:0:0,"));
        assert!(!vertical_filter(0).contains("crop=iw:"));
        assert!(vertical_filter(90).contains("100-40"));
    }
}
