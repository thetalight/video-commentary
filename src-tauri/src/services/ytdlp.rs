use crate::services::jobs;
use crate::services::task_events;
use crate::services::video_url::{self, VideoPlatform};
use regex::Regex;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const YTDLP_STALL_TIMEOUT: Duration = Duration::from_secs(90);

/// Upper bound on how often a download progress line reaches the UI.
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_secs(1);

/// `|` separated so both the percentage and the human-facing detail can be read
/// from one line, whatever the terminal width would have been.
const PROGRESS_TEMPLATE: &str =
    "download:[download] %(progress._percent_str)s|%(progress._total_bytes_str)s|%(progress._speed_str)s|%(progress._eta_str)s";

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
        // 采集站的画质由播放页决定，交给 yt-dlp 自己挑；m3u8 落盘后统一 recode 成 mp4。
        VideoPlatform::Yfsp | VideoPlatform::Nnyy => (None, "mp4", true),
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

/// Raw yt-dlp chatter: keep the plugin's own guidance and real errors, drop the
/// `[info]` noise and our machine-readable `VC_SUBS=` line so they never
/// overwrite the Chinese status text.
fn is_reportable_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && !trimmed.starts_with("[info]")
        && !trimmed.starts_with("VC_SUBS=")
        && !trimmed.starts_with(FILEPATH_PREFIX)
        && !trimmed.starts_with(TITLE_PREFIX)
}

fn printed_value<'a>(lines: &[&'a str], prefix: &str) -> Option<&'a str> {
    lines
        .iter()
        .find_map(|line| line.trim().strip_prefix(prefix))
        .map(str::trim)
}

fn format_sort_for_quality(quality: &str) -> &'static str {
    match quality {
        "1080" => "res:1080,codec:h264",
        "720" => "res:720,codec:h264",
        _ => "res,codec:h264",
    }
}

fn parse_percent(field: &str) -> Option<f64> {
    field
        .trim()
        .trim_end_matches('%')
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

/// Placeholder fields such as `N/A`, `Unknown` or `---.-%` carry no information.
fn progress_field(field: &str) -> Option<&str> {
    let trimmed = field.trim();
    let lowered = trimmed.to_ascii_lowercase();
    if trimmed.is_empty()
        || lowered == "n/a"
        || lowered == "unknown"
        || lowered.contains("---")
        || lowered.ends_with("b/s") && lowered.starts_with("0.00")
    {
        None
    } else {
        Some(trimmed)
    }
}

struct ProgressLine {
    percent: Option<f64>,
    message: String,
}

/// Reads one line produced by [`PROGRESS_TEMPLATE`]:
/// `[download]  42.3%|3.81MiB|699.32KiB/s|00:04`
fn parse_progress_line(line: &str) -> Option<ProgressLine> {
    let body = line.trim().strip_prefix("[download]")?.trim();
    let fields: Vec<&str> = body.split('|').collect();
    let percent = fields
        .first()
        .and_then(|field| parse_percent(field))
        .or_else(|| parse_percent_legacy(body));

    let mut detail = Vec::new();
    if let Some(total) = fields.get(1).and_then(|field| progress_field(field)) {
        detail.push(total.to_string());
    }
    if let Some(speed) = fields.get(2).and_then(|field| progress_field(field)) {
        detail.push(speed.to_string());
    }
    if let Some(eta) = fields.get(3).and_then(|field| progress_field(field)) {
        detail.push(format!("剩余 {eta}"));
    }

    let message = match percent {
        Some(percent) => {
            let mut text = format!("正在下载 {percent:.1}%");
            if !detail.is_empty() {
                text.push_str(&format!("（{}）", detail.join(" · ")));
            }
            text
        }
        None if !detail.is_empty() => format!("正在下载（{}）", detail.join(" · ")),
        None => "正在下载...".to_string(),
    };
    Some(ProgressLine { percent, message })
}

/// Fallback for templates that only carry the default `x.y% of ...` shape.
fn parse_percent_legacy(body: &str) -> Option<f64> {
    let re = Regex::new(r"([\d.]+)%").ok()?;
    re.captures(body)
        .and_then(|captures| captures.get(1))
        .and_then(|matched| matched.as_str().parse().ok())
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
        "--no-colors".to_string(),
        // `--print` implies `--quiet`, and `--quiet` implies `--no-progress`, so the
        // whole download runs without a single progress line: the UI stays at 0% and
        // the stall watchdog kills a perfectly healthy download.  Ask for the progress
        // bar explicitly; `--newline` keeps it as plain lines on stdout.
        "--progress".to_string(),
        "--progress-template".to_string(),
        PROGRESS_TEMPLATE.to_string(),
        "-o".to_string(),
        output_str.to_string(),
    ];

    // 采集站的 m3u8 分片同样适合并发拉取，播放页解析耗时也更短。
    if matches!(platform, VideoPlatform::Yfsp | VideoPlatform::Nnyy) {
        args.extend(["--concurrent-fragments".to_string(), "4".to_string()]);
    }

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

/// Total bytes currently on disk for a job directory. Used as a second liveness
/// signal: data can keep flowing even when yt-dlp prints nothing for a while.
fn directory_bytes(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.metadata().ok())
                .filter(|meta| meta.is_file())
                .map(|meta| meta.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Each persisted stage has its own 0–100% progress range.
#[derive(Clone, Copy)]
struct ProgressWindow {
    from: f64,
    to: f64,
    stage: &'static str,
}

impl ProgressWindow {
    const VIDEO: Self = Self {
        from: 0.0,
        to: 100.0,
        stage: "download",
    };
    const SUBTITLE: Self = Self {
        from: 0.0,
        to: 100.0,
        stage: "download_subtitle",
    };

    fn start(&self) -> f64 {
        self.from
    }

    fn stage(&self) -> &'static str {
        self.stage
    }

    fn map(&self, percent: f64) -> f64 {
        self.from + (percent.clamp(0.0, 100.0) / 100.0) * (self.to - self.from)
    }
}

fn run_ytdlp_with_progress(
    app: &AppHandle,
    task_id: Option<&str>,
    args: &[String],
    window: ProgressWindow,
    watch_dir: Option<&Path>,
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

    enum StreamEvent {
        Line { stderr: bool, text: String },
        Finished,
    }

    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    let (sender, receiver) = mpsc::channel();
    let streams: [(bool, Box<dyn Read + Send>); 2] =
        [(false, Box::new(stdout)), (true, Box::new(stderr))];
    let readers = streams.map(|(is_stderr, stream)| {
        let sender = sender.clone();
        thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                let Ok(text) = line else {
                    break;
                };
                if sender
                    .send(StreamEvent::Line {
                        stderr: is_stderr,
                        text,
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = sender.send(StreamEvent::Finished);
        })
    });
    drop(sender);

    let mut stdout_lines = Vec::new();
    let mut stderr_lines = Vec::new();
    let mut last_percent = window.start();
    let mut emitted_percent = last_percent;
    let mut last_emit = Instant::now();
    let mut last_activity = Instant::now();
    let mut watched_bytes = watch_dir.map(directory_bytes).unwrap_or(0);
    let mut finished_streams = 0;
    let mut stalled = false;

    while finished_streams < 2 {
        match receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(StreamEvent::Line { stderr, text }) => {
                last_activity = Instant::now();
                if stderr {
                    stderr_lines.push(text.clone());
                } else {
                    stdout_lines.push(text.clone());
                }
                match parse_progress_line(&text) {
                    Some(progress) => {
                        if let Some(percent) = progress.percent {
                            // Several streams (video, then audio) restart their own
                            // counter; the bar must not walk backwards.
                            last_percent = last_percent.max(window.map(percent)).min(window.to);
                        }
                        // A fast link emits dozens of lines per second and every emit
                        // costs a database write plus a job.json rewrite, so only
                        // speak up when the bar actually moves or a second has passed.
                        let moved = (last_percent - emitted_percent).abs() >= 0.5;
                        if moved || last_emit.elapsed() >= PROGRESS_EMIT_INTERVAL {
                            emitted_percent = last_percent;
                            last_emit = Instant::now();
                            task_events::emit_progress(
                                app,
                                task_id,
                                window.stage(),
                                last_percent,
                                progress.message,
                            );
                        }
                    }
                    None => {
                        if is_reportable_line(&text) {
                            task_events::emit_progress(
                                app,
                                task_id,
                                window.stage(),
                                last_percent,
                                humanize_ytdlp_line(&text),
                            );
                        }
                    }
                }
            }
            Ok(StreamEvent::Finished) => finished_streams += 1,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(dir) = watch_dir {
                    let bytes = directory_bytes(dir);
                    if bytes > watched_bytes {
                        watched_bytes = bytes;
                        last_activity = Instant::now();
                    }
                }
                if last_activity.elapsed() >= YTDLP_STALL_TIMEOUT {
                    stalled = true;
                    let _ = child.kill();
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let status = child.wait()?;
    for reader in readers {
        let _ = reader.join();
    }
    while let Ok(event) = receiver.try_recv() {
        if let StreamEvent::Line { stderr, text } = event {
            if stderr {
                stderr_lines.push(text);
            } else {
                stdout_lines.push(text);
            }
        }
    }

    let output = Output {
        status,
        stdout: stdout_lines.join("\n").into_bytes(),
        stderr: stderr_lines.join("\n").into_bytes(),
    };
    if stalled {
        return Err(YtdlpError::Failed(
            "yt-dlp download stalled for 90 seconds".to_string(),
        ));
    }
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

fn is_download_stall_error(msg: &str) -> bool {
    msg.contains("download stalled for 90 seconds")
}

fn is_transient_network_error(msg: &str) -> bool {
    msg.contains("SSL_connect")
        || msg.contains("SSL_ERROR_SYSCALL")
        || msg.contains("UNEXPECTED_EOF_WHILE_READING")
        || msg.contains("EOF occurred in violation of protocol")
        || msg.contains("Connection closed abruptly")
        || msg.contains("Connection reset by peer")
        || msg.contains("Remote end closed connection")
}

fn is_output_dir_error(msg: &str) -> bool {
    msg.contains("Operation not permitted")
        && !is_cookie_access_error(msg)
        && (msg.contains("/Downloads/") || msg.contains(".vtt") || msg.contains(".mp4"))
}

/// yt-dlp's generic fallback notice always precedes the real error and tells the
/// user nothing actionable.  Plugin errors keep their Chinese body but lose the
/// `ERROR: [nnyy] 20080573-1-1:` prefix, which only carries internal ids.
fn strip_known_noise(combined: &str) -> String {
    combined
        .lines()
        .filter(|line| {
            !line
                .trim_start()
                .starts_with("WARNING: [generic] Falling back on generic information extractor")
        })
        .map(|line| {
            let trimmed = line.trim_start();
            match trimmed.strip_prefix("ERROR: [nnyy] ") {
                Some(rest) => rest.split_once(": ").map_or(rest, |(_, message)| message),
                None => line,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_ytdlp_error(combined: &str) -> YtdlpError {
    let combined = &strip_known_noise(combined);
    let hint = if is_cookie_access_error(combined) {
        "\n\n提示: 应用无法读取所选浏览器的 Cookie。macOS 上的一帆视频会改用独立的 Chrome 验证窗口，不读取你的日常 Chrome Cookie 数据库。"
    } else if combined.contains("YFSP verification required")
        || combined.contains("YFSP Chrome verification")
        || combined.contains("Just a moment")
        || combined.contains("Cloudflare")
        || (combined.contains("[yfsp]") && combined.contains("403"))
        || (combined.contains("[generic]") && combined.contains("403"))
    {
        "\n\n提示: 请在应用弹出的专用 Chrome 窗口中完成人机验证。验证状态会保存在独立配置中，后续下载会自动复用。"
    } else if combined.contains("Impersonate target") {
        "\n\n提示: 当前 yt-dlp 缺少浏览器模拟支持。请升级 yt-dlp，或安装带 curl_cffi 的版本。"
    } else if is_transient_network_error(combined) {
        "\n\n提示: 代理与一帆服务器的 TLS 连接被临时断开。应用会自动重试；如果持续出现，请切换代理节点后再试。"
    } else if combined.contains("429") || combined.contains("Too Many Requests") {
        "\n\n提示: YouTube 限流。可尝试 Chrome Cookie，或运行: brew upgrade yt-dlp"
    } else if is_output_dir_error(combined) {
        "\n\n提示: yt-dlp 无法写入输出目录。请点击「默认」重置路径，或用 Browse 选择其他文件夹。"
    } else if combined.contains("Unsupported URL") {
        "\n\n提示: 这个页面里没有可直接下载的视频流。常见原因是：这是剧集列表/详情页（一页挂多集）、播放器需要 JS 才能拿到地址，或站点是采集站的多线路页。请粘贴具体某一集的播放页或视频直链；也支持 YouTube、Bilibili、X、一帆视频等平台。"
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
    window: ProgressWindow,
    watch_dir: Option<&Path>,
) -> Result<Output, YtdlpError> {
    match run_ytdlp_with_progress(app, task_id, args, window, watch_dir) {
        Ok(output) => Ok(output),
        Err(YtdlpError::Failed(msg)) if is_download_stall_error(&msg) => {
            task_events::emit_progress(
                app,
                task_id,
                window.stage(),
                window.start(),
                "网络分片连续 90 秒无响应，正在从断点自动重试...",
            );
            run_ytdlp_with_progress(app, task_id, args, window, watch_dir)
        }
        Err(YtdlpError::Failed(msg)) if is_transient_network_error(&msg) => {
            task_events::emit_progress(
                app,
                task_id,
                window.stage(),
                window.start(),
                "一帆服务器的 TLS 连接中断，正在自动重试...",
            );
            run_ytdlp_with_progress(app, task_id, args, window, watch_dir)
        }
        Err(YtdlpError::Failed(msg)) if is_cookie_access_error(&msg) => {
            task_events::emit_progress(
                app,
                task_id,
                window.stage(),
                window.start(),
                "无法读取浏览器 Cookie，正在不使用 Cookie 重试...",
            );
            let without = strip_cookies_args(args);
            run_ytdlp_with_progress(app, task_id, &without, window, watch_dir)
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
    // Current Chromium releases on macOS may store their cookie key in Apple
    // Passwords, where yt-dlp cannot decrypt it.  The YFSP extractor uses a
    // dedicated Chrome profile and CDP on macOS instead.
    if cfg!(target_os = "macos") && platform == VideoPlatform::Yfsp {
        return None;
    }
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

/// yt-dlp 的站点解析插件（一帆、努努影院）都放在同一个 `ytdlp_plugins` 目录下。
fn push_extractor_plugin_args(app: &AppHandle, args: &mut Vec<String>) -> Result<(), YtdlpError> {
    let plugin_dir = ytdlp_plugin_dir(app)
        .ok_or_else(|| YtdlpError::Failed("找不到站点解析插件".to_string()))?;

    args.extend([
        "--plugin-dirs".to_string(),
        plugin_dir.to_string_lossy().into_owned(),
    ]);
    if let Some(user_agent) = installed_chrome_user_agent() {
        args.extend(["--user-agent".to_string(), user_agent]);
    }
    Ok(())
}

/// Video-stage print so the subtitle pass reports which languages the platform
/// actually offers: yt-dlp's own "no subtitles for the requested languages" line
/// is identical whether the platform has none or simply none we asked for.
const SUBTITLE_LANGUAGES_PRINT: &str = "video:VC_SUBS=%(subtitles)j";

/// Marker prefixes for the two values read back from a download. Markers keep the
/// parsing immune to progress lines and to `[info]` chatter on stdout.
const FILEPATH_PREFIX: &str = "VC_FILE=";
const TITLE_PREFIX: &str = "VC_TITLE=";

/// What the subtitle pass found, including the platform's own language list.
pub struct SubtitleFetch {
    pub path: Option<PathBuf>,
    /// Empty means the platform offers no standalone subtitles at all.
    pub platform_languages: Vec<String>,
    pub error: Option<String>,
}

impl SubtitleFetch {
    fn empty() -> Self {
        Self {
            path: None,
            platform_languages: Vec::new(),
            error: None,
        }
    }
}

fn parse_subtitle_languages(stdout: &str) -> Vec<String> {
    let Some(raw) = stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("VC_SUBS="))
    else {
        return Vec::new();
    };
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw.trim())
    else {
        return Vec::new();
    };
    let mut languages: Vec<String> = map.keys().cloned().collect();
    languages.sort();
    languages
}

fn downloaded_subtitle_path(attempt_dir: &Path, succeeded: bool) -> Option<PathBuf> {
    succeeded
        .then(|| jobs::find_subtitle_in_dir(attempt_dir, "source"))
        .flatten()
}

fn download_subtitles_for_target(
    app: &AppHandle,
    platform: VideoPlatform,
    download_url: &str,
    playlist_args: &[String],
    job_dir: &Path,
    options: &DownloadOptions,
) -> Result<SubtitleFetch, YtdlpError> {
    if platform == VideoPlatform::Twitter {
        return Ok(SubtitleFetch::empty());
    }

    // A retry must only inspect subtitles produced by this exact invocation.
    // Previous sidecars may belong to another source or an earlier failed run.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let attempt_dir = job_dir
        .join("subtitle-attempts")
        .join(format!("{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&attempt_dir)?;
    let output_template = attempt_dir.join("source.%(ext)s");
    let output_str = output_template.to_string_lossy();
    let task_id = options.task_id.as_deref();
    let cookies = cookies_browser_for_platform(platform, options.cookies_browser.as_deref());
    let mut sub_args = base_args(&output_str, platform, &options.video_quality);
    sub_args.extend([
        "--skip-download".to_string(),
        "--write-subs".to_string(),
        "--write-auto-subs".to_string(),
        // `--print` would otherwise silence the plugin's verification prompts, and
        // those are the only guidance the user gets while the Chrome window is up.
        "--no-quiet".to_string(),
        "--print".to_string(),
        SUBTITLE_LANGUAGES_PRINT.to_string(),
        "--sub-langs".to_string(),
        // Anchor the codes: `zh.*` also matches hundreds of auto-translated
        // variants such as zh-Hans-ar, making one short download take minutes.
        "^zh-Hans$,^zh-CN$,^zh$,^zh-Hant$,^zh-Hans-zh-CN$,^en$,^en-orig$,^en-US$,^en-GB$,^ja$,^ko$"
            .to_string(),
        "--sub-format".to_string(),
        "srt/best".to_string(),
        "--no-update".to_string(),
    ]);
    if matches!(platform, VideoPlatform::Yfsp | VideoPlatform::Nnyy) {
        push_extractor_plugin_args(app, &mut sub_args)?;
    }
    push_cookies_args(&mut sub_args, cookies);
    sub_args.extend(playlist_args.iter().cloned());
    sub_args.push(download_url.to_string());

    task_events::emit_progress(
        app,
        task_id,
        "download_subtitle",
        0.0,
        "正在拉取平台字幕...",
    );
    let subtitle_result = run_ytdlp_with_progress_retry(
        app,
        task_id,
        &sub_args,
        ProgressWindow::SUBTITLE,
        Some(&attempt_dir),
    );
    let mut platform_languages = Vec::new();
    let mut error = None;
    match subtitle_result {
        Ok(output) => {
            platform_languages = parse_subtitle_languages(&String::from_utf8_lossy(&output.stdout));
        }
        Err(failure) => {
            eprintln!("Subtitle download failed (non-fatal): {failure}");
            error = Some(failure.to_string());
        }
    }

    let path = downloaded_subtitle_path(&attempt_dir, error.is_none());
    if path.is_none() {
        let _ = std::fs::remove_dir_all(&attempt_dir);
    }
    Ok(SubtitleFetch {
        path,
        platform_languages,
        error,
    })
}

/// Fetch only the platform-provided subtitle for an existing task.
pub fn download_subtitles_into_job(
    app: &AppHandle,
    url: &str,
    job_dir: &Path,
    options: DownloadOptions,
) -> Result<SubtitleFetch, YtdlpError> {
    let platform = validate_video_url(url)?;
    let target = match platform {
        VideoPlatform::Youtube => video_url::resolve_youtube_download(url),
        VideoPlatform::Yfsp => video_url::resolve_yfsp_download(url).map_err(YtdlpError::Failed)?,
        VideoPlatform::Twitter | VideoPlatform::Nnyy | VideoPlatform::Generic => {
            video_url::VideoDownloadTarget {
                url: url.to_string(),
                extra_args: Vec::new(),
            }
        }
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
        VideoPlatform::Twitter | VideoPlatform::Nnyy | VideoPlatform::Generic => {
            video_url::VideoDownloadTarget {
                url: url.to_string(),
                extra_args: Vec::new(),
            }
        }
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
            VideoPlatform::Nnyy => "正在解析努努影院播放页...",
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
        format!("after_move:{FILEPATH_PREFIX}%(filepath)s"),
        "--print".to_string(),
        format!("after_move:{TITLE_PREFIX}%(title)s"),
    ]);
    if recode_video {
        video_args.extend(["--recode-video".to_string(), "mp4".to_string()]);
    }
    if matches!(platform, VideoPlatform::Yfsp | VideoPlatform::Nnyy) {
        push_extractor_plugin_args(app, &mut video_args)?;
    }
    video_args.extend(playlist_args.clone());
    push_cookies_args(&mut video_args, cookies);
    video_args.push(download_url.to_string());

    let video_output = run_ytdlp_with_progress_retry(
        app,
        task_id,
        &video_args,
        ProgressWindow::VIDEO,
        Some(job_dir),
    )?;
    let stdout = String::from_utf8_lossy(&video_output.stdout);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();

    let video_path = printed_value(&lines, FILEPATH_PREFIX)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| {
            lines
                .iter()
                .find(|line| {
                    let lower = line.to_lowercase();
                    lower.ends_with(".mp4") || lower.ends_with(".webm") || lower.ends_with(".mkv")
                })
                .map(PathBuf::from)
        })
        .or_else(|| find_latest_video(job_dir))
        .ok_or_else(|| YtdlpError::Failed("无法确定已下载的视频路径".into()))?;

    let title = printed_value(&lines, TITLE_PREFIX)
        .filter(|title| !title.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            lines
                .iter()
                .rev()
                .find(|line| {
                    !Path::new(line).exists()
                        && !line.starts_with('[')
                        && !line.contains('|')
                        && !line.contains('=')
                })
                .map(|line| line.to_string())
        })
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

    task_events::emit_progress(app, task_id, "download", 100.0, "原片下载完成");

    Ok(DownloadOutput {
        video_path: canonical,
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
    fn subtitle_retry_never_selects_an_older_attempt() {
        let root = std::env::temp_dir().join(format!(
            "vc-subtitle-attempt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let old = root.join("old");
        let fresh = root.join("fresh");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::write(old.join("source.zh.srt"), b"old").unwrap();
        assert!(downloaded_subtitle_path(&fresh, true).is_none());
        std::fs::write(fresh.join("source.en.srt"), b"fresh").unwrap();
        assert_eq!(
            downloaded_subtitle_path(&fresh, true),
            Some(fresh.join("source.en.srt"))
        );
        assert!(downloaded_subtitle_path(&fresh, false).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

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
        let expected_default = if cfg!(target_os = "macos") {
            None
        } else {
            Some("chrome")
        };
        let expected_configured = if cfg!(target_os = "macos") {
            None
        } else {
            Some("brave")
        };
        assert_eq!(
            cookies_browser_for_platform(VideoPlatform::Yfsp, None),
            expected_default
        );
        assert_eq!(
            cookies_browser_for_platform(VideoPlatform::Yfsp, Some("brave")),
            expected_configured
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

    /// `--print` implies `--quiet`, which implies `--no-progress`; without an
    /// explicit `--progress` the download reports nothing and the stall watchdog
    /// kills it. Guard the flag so the fix cannot be dropped by accident.
    #[test]
    fn progress_stays_enabled_even_though_print_implies_quiet() {
        let args = base_args("source.%(ext)s", VideoPlatform::Yfsp, "best");

        assert!(args.iter().any(|arg| arg == "--progress"));
        assert!(!args.iter().any(|arg| arg == "--no-progress"));
        assert!(args.iter().any(|arg| arg == "--newline"));
        assert!(args.iter().any(|arg| arg == PROGRESS_TEMPLATE));
    }

    #[test]
    fn progress_lines_carry_percentage_and_detail() {
        let line = parse_progress_line("[download]  42.3%|  3.81MiB|699.32KiB/s|00:04").unwrap();

        assert_eq!(line.percent, Some(42.3));
        assert_eq!(
            line.message,
            "正在下载 42.3%（3.81MiB · 699.32KiB/s · 剩余 00:04）"
        );
    }

    #[test]
    fn fragment_downloads_with_unknown_size_still_report_percent() {
        let line =
            parse_progress_line("[download]   1.4%|        N/A|   1.12MiB/s|Unknown").unwrap();

        assert_eq!(line.percent, Some(1.4));
        assert_eq!(line.message, "正在下载 1.4%（1.12MiB/s）");
    }

    #[test]
    fn non_progress_lines_are_ignored() {
        assert!(parse_progress_line("source.mp4").is_none());
        assert!(parse_progress_line("[info] Downloading subtitles").is_none());
    }

    #[test]
    fn progress_windows_keep_the_bar_moving_forward() {
        assert_eq!(ProgressWindow::VIDEO.map(0.0), 0.0);
        assert_eq!(ProgressWindow::VIDEO.map(100.0), 100.0);
        assert_eq!(ProgressWindow::SUBTITLE.map(0.0), 0.0);
        assert_eq!(ProgressWindow::SUBTITLE.map(100.0), 100.0);
    }

    #[test]
    fn platform_subtitle_languages_are_read_from_the_print_line() {
        let stdout = concat!(
            "[yfsp] pEw1EI8SQe6: Reading YFSP player configuration\n",
            "VC_SUBS={\"vi\": [], \"zh-Hans\": [{\"url\": \"https://example.com/a.vtt\", \"ext\": \"vtt\"}]}\n",
            "source.mp4\n"
        );

        assert_eq!(
            parse_subtitle_languages(stdout),
            vec!["vi".to_string(), "zh-Hans".to_string()]
        );
    }

    #[test]
    fn a_platform_without_subtitles_reports_no_language() {
        assert!(parse_subtitle_languages("VC_SUBS=NA").is_empty());
        assert!(parse_subtitle_languages("VC_SUBS={}").is_empty());
        assert!(parse_subtitle_languages("source.mp4").is_empty());
    }

    #[test]
    fn ytdlp_chatter_does_not_overwrite_the_status_message() {
        assert!(!is_reportable_line(
            "[info] There are no subtitles for the requested languages"
        ));
        assert!(!is_reportable_line("VC_SUBS=NA"));
        assert!(is_reportable_line(
            "[yfsp] pEw1EI8SQe6: Chrome verification window opened; complete the check if prompted"
        ));
        assert!(is_reportable_line(
            "ERROR: [yfsp] pEw1EI8SQe6: Unable to start the YFSP Chrome verification session"
        ));
    }

    #[test]
    fn unsupported_pages_explain_themselves_instead_of_blaming_ytdlp() {
        let raw = concat!(
            "WARNING: [generic] Falling back on generic information extractor\n",
            "ERROR: Unsupported URL: https://nnyy.in/dianshiju/20080573.html"
        );
        let YtdlpError::Failed(message) = format_ytdlp_error(raw) else {
            panic!("expected a failure");
        };

        assert!(!message.contains("Falling back on generic"));
        assert!(message.contains("没有可直接下载的视频流"));
        assert!(message.contains("剧集列表/详情页"));
    }

    #[test]
    fn plugin_errors_keep_their_chinese_body_without_internal_ids() {
        let raw = "ERROR: [nnyy] 20080573: 这个链接是剧集列表页（共 24 集），请粘贴具体一集的地址";
        let YtdlpError::Failed(message) = format_ytdlp_error(raw) else {
            panic!("expected a failure");
        };

        assert!(message.starts_with("这个链接是剧集列表页"));
        assert!(!message.contains("[nnyy]"));
    }
}
