use crate::services::jobs;
use crate::services::task_events;
use crate::services::video_url::{self, VideoPlatform};
use regex::Regex;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tauri::{AppHandle, Manager};

#[derive(Debug, thiserror::Error)]
pub enum YtdlpError {
    #[error("yt-dlp not found in PATH")]
    NotFound,
    #[error("{0}")]
    Failed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct DownloadOutput {
    pub video_path: PathBuf,
    pub subtitle_path: Option<PathBuf>,
    pub title: String,
}

pub struct DownloadOptions {
    pub cookies_browser: Option<String>,
    pub video_quality: String,
    pub task_id: Option<String>,
}

fn format_for_quality(quality: &str) -> (&'static str, &'static str) {
    match quality {
        "1080" => (
            "bestvideo[height<=1080][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=1080]+bestaudio/best[height<=1080]",
            "mp4",
        ),
        "720" => (
            "bestvideo[height<=720][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=720]+bestaudio/best[height<=720]",
            "mp4",
        ),
        _ => (
            "bestvideo[ext=mp4]+bestaudio[ext=m4a]/bestvideo+bestaudio/best",
            "mp4",
        ),
    }
}

fn format_for_platform(
    platform: VideoPlatform,
    quality: &str,
) -> (Option<&'static str>, &'static str, bool) {
    match platform {
        VideoPlatform::Twitter => (Some("best[ext=mp4]/best"), "mp4", false),
        VideoPlatform::Yfsp => (None, "mp4", true),
        VideoPlatform::Youtube | VideoPlatform::Generic => {
            let (format, merge_fmt) = format_for_quality(quality);
            (Some(format), merge_fmt, true)
        }
    }
}

fn humanize_ytdlp_line(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return "正在下载...".to_string();
    }
    trimmed.to_string()
}

fn format_sort_for_quality(quality: &str) -> &'static str {
    match quality {
        "1080" => "res:1080,codec:h264",
        "720" => "res:720,codec:h264",
        _ => "res,codec:h264",
    }
}

fn parse_progress(line: &str) -> Option<f64> {
    let re = Regex::new(r"\[download\]\s+([\d.]+)%").ok()?;
    re.captures(line)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
}

pub fn validate_video_url(url: &str) -> Result<VideoPlatform, YtdlpError> {
    video_url::validate_video_url(url).map_err(YtdlpError::Failed)
}

fn base_args(output_str: &str, platform: VideoPlatform, video_quality: &str) -> Vec<String> {
    let mut args = vec![
        "--no-config-locations".to_string(),
        "--restrict-filenames".to_string(),
        "--retries".to_string(),
        "10".to_string(),
        "--fragment-retries".to_string(),
        "10".to_string(),
        "--sleep-interval".to_string(),
        "2".to_string(),
        "--max-sleep-interval".to_string(),
        "8".to_string(),
        "--newline".to_string(),
        "-o".to_string(),
        output_str.to_string(),
    ];

    if platform == VideoPlatform::Youtube {
        args.extend([
            "--extractor-args".to_string(),
            "youtube:player_client=default".to_string(),
            "-S".to_string(),
            format_sort_for_quality(video_quality).to_string(),
        ]);
    }

    args
}

fn run_ytdlp_with_progress(
    app: &AppHandle,
    task_id: Option<&str>,
    args: &[String],
) -> Result<Output, YtdlpError> {
    let mut child = Command::new("yt-dlp")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                YtdlpError::NotFound
            } else {
                YtdlpError::Io(e)
            }
        })?;

    let stderr = child.stderr.take().expect("stderr");
    let reader = BufReader::new(stderr);
    let mut stderr_lines = Vec::new();
    let mut last_percent = 0.0_f64;

    for line in reader.lines() {
        let line = line?;
        stderr_lines.push(line.clone());
        if let Some(percent) = parse_progress(&line) {
            last_percent = percent;
        }
        task_events::emit_progress(
            app,
            task_id,
            "download",
            last_percent,
            humanize_ytdlp_line(&line),
        );
    }

    let output = child.wait_with_output()?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let combined = if err.is_empty() {
            stderr_lines.join("\n")
        } else {
            err.to_string()
        };
        return Err(format_ytdlp_error(&combined));
    }

    Ok(output)
}

fn is_cookie_access_error(msg: &str) -> bool {
    msg.contains("Cookies.binarycookies")
        || msg.contains("com.apple.Safari")
        || msg.contains("cookies-from-browser")
}

fn is_output_dir_error(msg: &str) -> bool {
    msg.contains("Operation not permitted")
        && !is_cookie_access_error(msg)
        && (msg.contains("/Downloads/") || msg.contains(".vtt") || msg.contains(".mp4"))
}

fn format_ytdlp_error(combined: &str) -> YtdlpError {
    let hint = if is_cookie_access_error(combined) {
        "\n\n提示: 应用无法读取浏览器 Cookie。使用一帆视频时会自动读取 Chrome Cookie；请允许系统的钥匙串访问提示，必要时完全退出 Chrome 后重试。"
    } else if combined.contains("YFSP verification required")
        || combined.contains("Just a moment")
        || combined.contains("Cloudflare")
        || (combined.contains("[yfsp]") && combined.contains("403"))
        || (combined.contains("[generic]") && combined.contains("403"))
    {
        "\n\n提示: 请先用 Chrome 打开该播放页并完成人机验证，然后直接重试。应用会自动复用 Chrome Cookie，无需每条视频重复设置。"
    } else if combined.contains("Impersonate target") {
        "\n\n提示: 当前 yt-dlp 缺少浏览器模拟支持。请升级 yt-dlp，或安装带 curl_cffi 的版本。"
    } else if combined.contains("429") || combined.contains("Too Many Requests") {
        "\n\n提示: YouTube 限流。可尝试 Chrome Cookie，或运行: brew upgrade yt-dlp"
    } else if is_output_dir_error(combined) {
        "\n\n提示: yt-dlp 无法写入输出目录。请点击「默认」重置路径，或用 Browse 选择其他文件夹。"
    } else if combined.contains("Unsupported URL") {
        "\n\n提示: yt-dlp 无法解析这个网站。请换用它支持的平台链接（YouTube、Bilibili、X 等），或运行: brew upgrade yt-dlp"
    } else if combined.contains("Unable to download") || combined.contains("twitter") {
        "\n\n提示: 该站点下载失败。可检查网络，或在设置里换 Chrome Cookie 后重试。"
    } else {
        ""
    };
    YtdlpError::Failed(format!("{combined}{hint}"))
}

fn strip_cookies_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--cookies-from-browser" {
            skip_next = true;
            continue;
        }
        out.push(arg.clone());
    }
    out
}

fn run_ytdlp_with_progress_retry(
    app: &AppHandle,
    task_id: Option<&str>,
    args: &[String],
) -> Result<Output, YtdlpError> {
    match run_ytdlp_with_progress(app, task_id, args) {
        Ok(output) => Ok(output),
        Err(YtdlpError::Failed(msg)) if is_cookie_access_error(&msg) => {
            task_events::emit_progress(
                app,
                task_id,
                "download",
                0.0,
                "无法读取浏览器 Cookie，正在不使用 Cookie 重试...",
            );
            let without = strip_cookies_args(args);
            run_ytdlp_with_progress(app, task_id, &without)
        }
        Err(e) => Err(e),
    }
}

fn push_cookies_args(args: &mut Vec<String>, browser: Option<&str>) {
    let browser = browser.filter(|b| !b.is_empty() && !b.eq_ignore_ascii_case("safari"));
    if let Some(browser) = browser {
        args.push("--cookies-from-browser".to_string());
        args.push(browser.to_string());
    }
}

fn cookies_browser_for_platform<'a>(
    platform: VideoPlatform,
    configured: Option<&'a str>,
) -> Option<&'a str> {
    match configured.filter(|browser| !browser.trim().is_empty()) {
        Some(browser) => Some(browser),
        None if platform == VideoPlatform::Yfsp => Some("chrome"),
        None => None,
    }
}

fn ytdlp_plugin_dir(app: &AppHandle) -> Option<PathBuf> {
    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ytdlp_plugins");
    if development.is_dir() {
        return Some(development);
    }

    app.path()
        .resource_dir()
        .ok()
        .map(|dir| dir.join("ytdlp_plugins"))
        .filter(|dir| dir.is_dir())
}

fn chrome_user_agent_for_version(version: &str) -> Option<String> {
    let major = version.trim().split('.').next()?.parse::<u16>().ok()?;
    if major < 80 {
        return None;
    }
    Some(format!(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"
    ))
}

fn installed_chrome_user_agent() -> Option<String> {
    let output = Command::new("/usr/bin/plutil")
        .args([
            "-extract",
            "CFBundleShortVersionString",
            "raw",
            "/Applications/Google Chrome.app/Contents/Info.plist",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    chrome_user_agent_for_version(&String::from_utf8_lossy(&output.stdout))
}

fn push_yfsp_extractor_args(app: &AppHandle, args: &mut Vec<String>) -> Result<(), YtdlpError> {
    let plugin_dir = ytdlp_plugin_dir(app)
        .ok_or_else(|| YtdlpError::Failed("找不到一帆视频解析插件".to_string()))?;

    args.extend([
        "--plugin-dirs".to_string(),
        plugin_dir.to_string_lossy().into_owned(),
        "--impersonate".to_string(),
        "chrome".to_string(),
    ]);
    if let Some(user_agent) = installed_chrome_user_agent() {
        args.extend(["--user-agent".to_string(), user_agent]);
    }
    Ok(())
}

fn download_subtitles_for_target(
    app: &AppHandle,
    platform: VideoPlatform,
    download_url: &str,
    playlist_args: &[String],
    job_dir: &Path,
    options: &DownloadOptions,
) -> Result<Option<PathBuf>, YtdlpError> {
    if platform == VideoPlatform::Twitter {
        return Ok(None);
    }

    std::fs::create_dir_all(job_dir)?;
    let output_template = job_dir.join("source.%(ext)s");
    let output_str = output_template.to_string_lossy();
    let task_id = options.task_id.as_deref();
    let cookies = cookies_browser_for_platform(platform, options.cookies_browser.as_deref());
    let mut sub_args = base_args(&output_str, platform, &options.video_quality);
    sub_args.extend([
        "--skip-download".to_string(),
        "--write-subs".to_string(),
        "--write-auto-subs".to_string(),
        "--sub-langs".to_string(),
        "zh-Hans.*,zh-CN.*,zh.*,en.*,ja.*,ko.*".to_string(),
        "--sub-format".to_string(),
        "srt/best".to_string(),
        "--no-update".to_string(),
    ]);
    if platform == VideoPlatform::Yfsp {
        push_yfsp_extractor_args(app, &mut sub_args)?;
    }
    push_cookies_args(&mut sub_args, cookies);
    sub_args.extend(playlist_args.iter().cloned());
    sub_args.push(download_url.to_string());

    task_events::emit_progress(app, task_id, "download", 95.0, "正在拉取平台字幕...");
    let subtitle_result = if platform == VideoPlatform::Yfsp {
        run_ytdlp_with_progress(app, task_id, &sub_args)
    } else {
        run_ytdlp_with_progress_retry(app, task_id, &sub_args)
    };
    if let Err(error) = subtitle_result {
        eprintln!("Subtitle download failed (non-fatal): {error}");
    }

    let canonical = job_dir.join("source.mp4");
    Ok(jobs::relocate_downloaded_subtitles(&canonical, job_dir)
        .ok()
        .flatten()
        .or_else(|| jobs::find_subtitle_in_dir(job_dir, "source")))
}

/// Fetch only the platform-provided subtitle for an existing task.
pub fn download_subtitles_into_job(
    app: &AppHandle,
    url: &str,
    job_dir: &Path,
    options: DownloadOptions,
) -> Result<Option<PathBuf>, YtdlpError> {
    let platform = validate_video_url(url)?;
    let target = match platform {
        VideoPlatform::Youtube => video_url::resolve_youtube_download(url),
        VideoPlatform::Yfsp => video_url::resolve_yfsp_download(url).map_err(YtdlpError::Failed)?,
        VideoPlatform::Twitter | VideoPlatform::Generic => video_url::VideoDownloadTarget {
            url: url.to_string(),
            extra_args: Vec::new(),
        },
    };
    download_subtitles_for_target(
        app,
        platform,
        &target.url,
        &target.extra_args,
        job_dir,
        &options,
    )
}

/// Download into `job_dir` as `source.mp4` (plus optional sidecar subs).
pub fn download_into_job(
    app: &AppHandle,
    url: &str,
    job_dir: &Path,
    options: DownloadOptions,
) -> Result<DownloadOutput, YtdlpError> {
    let platform = validate_video_url(url)?;
    let download_target = match platform {
        VideoPlatform::Youtube => video_url::resolve_youtube_download(url),
        VideoPlatform::Yfsp => video_url::resolve_yfsp_download(url).map_err(YtdlpError::Failed)?,
        VideoPlatform::Twitter | VideoPlatform::Generic => video_url::VideoDownloadTarget {
            url: url.to_string(),
            extra_args: Vec::new(),
        },
    };
    let download_url = download_target.url.as_str();
    let playlist_args = download_target.extra_args.clone();

    std::fs::create_dir_all(job_dir)?;
    let output_template = job_dir.join("source.%(ext)s");
    let output_str = output_template.to_string_lossy();

    let task_id = options.task_id.as_deref();
    task_events::emit_progress(
        app,
        task_id,
        "download",
        0.0,
        match platform {
            VideoPlatform::Twitter => "正在解析 X/Twitter 链接...",
            VideoPlatform::Youtube => "正在解析 YouTube 链接...",
            VideoPlatform::Yfsp => "正在解析一帆视频正片...",
            VideoPlatform::Generic => "正在解析视频链接...",
        },
    );

    let (format, merge_fmt, recode_video) = format_for_platform(platform, &options.video_quality);
    // YFSP is protected by a browser challenge. Default to Chrome for this
    // platform so users do not have to change and save the global setting for
    // every download. An explicitly configured browser still takes priority.
    let cookies = cookies_browser_for_platform(platform, options.cookies_browser.as_deref());

    let mut video_args = base_args(&output_str, platform, &options.video_quality);
    if let Some(format) = format {
        video_args.extend(["-f".to_string(), format.to_string()]);
    }
    video_args.extend([
        "--merge-output-format".to_string(),
        merge_fmt.to_string(),
        "--no-write-subs".to_string(),
        "--no-update".to_string(),
        "--socket-timeout".to_string(),
        "30".to_string(),
        "--print".to_string(),
        "after_move:filepath".to_string(),
        "--print".to_string(),
        "after_move:title".to_string(),
    ]);
    if recode_video {
        video_args.extend(["--recode-video".to_string(), "mp4".to_string()]);
    }
    if platform == VideoPlatform::Yfsp {
        push_yfsp_extractor_args(app, &mut video_args)?;
    }
    video_args.extend(playlist_args.clone());
    push_cookies_args(&mut video_args, cookies);
    video_args.push(download_url.to_string());

    let video_output = if platform == VideoPlatform::Yfsp {
        run_ytdlp_with_progress(app, task_id, &video_args)?
    } else {
        run_ytdlp_with_progress_retry(app, task_id, &video_args)?
    };
    let stdout = String::from_utf8_lossy(&video_output.stdout);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();

    let video_path = lines
        .iter()
        .find(|l| {
            let lower = l.to_lowercase();
            lower.ends_with(".mp4") || lower.ends_with(".webm") || lower.ends_with(".mkv")
        })
        .map(PathBuf::from)
        .or_else(|| find_latest_video(job_dir))
        .ok_or_else(|| YtdlpError::Failed("无法确定已下载的视频路径".into()))?;

    let title = lines
        .iter()
        .rev()
        .find(|l| !Path::new(l).exists())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            video_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });

    let canonical = job_dir.join("source.mp4");
    if video_path != canonical {
        if canonical.exists() {
            let _ = std::fs::remove_file(&canonical);
        }
        if std::fs::rename(&video_path, &canonical).is_err() {
            std::fs::copy(&video_path, &canonical)?;
            let _ = std::fs::remove_file(&video_path);
        }
    }

    let subtitle_path = download_subtitles_for_target(
        app,
        platform,
        download_url,
        &playlist_args,
        job_dir,
        &options,
    )?;

    task_events::emit_progress(app, task_id, "download", 100.0, "下载完成");

    Ok(DownloadOutput {
        video_path: canonical,
        subtitle_path,
        title,
    })
}

fn find_latest_video(dir: &Path) -> Option<PathBuf> {
    let mut videos: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .map(|ext| {
                    let ext = ext.to_string_lossy().to_lowercase();
                    ext == "mp4" || ext == "webm" || ext == "mkv"
                })
                .unwrap_or(false)
        })
        .collect();

    videos.sort_by_key(|p| {
        p.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    videos.pop()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yfsp_lets_yt_dlp_pick_formats() {
        let (format, merge_format, recode) = format_for_platform(VideoPlatform::Yfsp, "best");

        assert_eq!(format, None);
        assert_eq!(merge_format, "mp4");
        assert!(recode);
    }

    #[test]
    fn yfsp_plugin_dir_contains_a_plugin_package() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ytdlp_plugins");
        let extractor = dir
            .join("yfsp")
            .join("yt_dlp_plugins")
            .join("extractor")
            .join("yfsp.py");
        assert!(extractor.is_file(), "{}", extractor.display());
    }

    #[test]
    fn yfsp_defaults_to_chrome_cookies() {
        assert_eq!(
            cookies_browser_for_platform(VideoPlatform::Yfsp, None),
            Some("chrome")
        );
        assert_eq!(
            cookies_browser_for_platform(VideoPlatform::Yfsp, Some("brave")),
            Some("brave")
        );
        assert_eq!(
            cookies_browser_for_platform(VideoPlatform::Youtube, None),
            None
        );
    }

    #[test]
    fn yfsp_uses_the_installed_chrome_major_version_for_cookie_fingerprint() {
        let user_agent = chrome_user_agent_for_version("152.0.7977.65").unwrap();

        assert!(user_agent.contains("Macintosh"));
        assert!(user_agent.contains("Chrome/152.0.0.0"));
        assert!(chrome_user_agent_for_version("not-a-version").is_none());
    }
}
