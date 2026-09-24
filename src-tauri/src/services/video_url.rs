#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoPlatform {
    Youtube,
    Twitter,
    Yfsp,
    Generic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoDownloadTarget {
    pub url: String,
    pub extra_args: Vec<String>,
}

pub fn detect_platform(url: &str) -> Option<VideoPlatform> {
    let trimmed = url.trim();
    let lower = trimmed.to_lowercase();
    if lower.contains("youtube.com") || lower.contains("youtu.be") {
        return Some(VideoPlatform::Youtube);
    }
    if is_twitter_url(&lower) {
        return Some(VideoPlatform::Twitter);
    }
    if is_yfsp_play_url(trimmed) || is_yfsp_master_playlist_url(trimmed) {
        return Some(VideoPlatform::Yfsp);
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(VideoPlatform::Generic);
    }
    None
}

fn is_twitter_url(lower: &str) -> bool {
    lower.contains("x.com/")
        || lower.contains("twitter.com/")
        || lower.contains("mobile.twitter.com/")
}

fn is_yfsp_play_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };

    (host.eq_ignore_ascii_case("yfsp.tv") || host.to_ascii_lowercase().ends_with(".yfsp.tv"))
        && parsed.path().starts_with("/play/")
}

fn normalize_yfsp_play_url(url: &str) -> Option<String> {
    let mut parsed = url::Url::parse(url.trim()).ok()?;
    let host = parsed.host_str()?;
    if !(host.eq_ignore_ascii_case("yfsp.tv") || host.to_ascii_lowercase().ends_with(".yfsp.tv")) {
        return None;
    }
    let raw_id = parsed.path().strip_prefix("/play/")?;
    let video_id = raw_id
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .collect::<String>();
    if video_id.is_empty() {
        return None;
    }
    parsed.set_path(&format!("/play/{video_id}"));
    parsed.set_fragment(None);
    Some(parsed.to_string())
}

fn is_yfsp_master_playlist_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };

    parsed
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("upload.yfsp.tv"))
        && parsed
            .path()
            .eq_ignore_ascii_case("/api/video/MasterPlayList")
}

pub fn validate_video_url(url: &str) -> Result<VideoPlatform, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("请输入视频链接".into());
    }
    if url.contains("yt-dlp failed") || url.contains("is not a valid URL") {
        return Err("URL 输入框里似乎是错误信息，请粘贴视频链接（以 https:// 开头）".into());
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(format!("无效的 URL，请以 https:// 开头: {url}"));
    }

    detect_platform(url).ok_or_else(|| "请输入以 https:// 开头的视频链接".into())
}

fn query_param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    for segment in url.split(['?', '&']) {
        if let Some(value) = segment.strip_prefix(&prefix) {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn playlist_only_url(list_id: &str) -> String {
    format!("https://www.youtube.com/playlist?list={list_id}")
}

pub fn resolve_youtube_download(url: &str) -> VideoDownloadTarget {
    let trimmed = url.trim();
    let list_id = query_param(trimmed, "list");
    let video_id = query_param(trimmed, "v");
    let index = query_param(trimmed, "index").and_then(|raw| raw.parse::<u32>().ok());

    if let Some(list_id) = list_id {
        if let Some(index) = index.filter(|n| *n > 0) {
            return VideoDownloadTarget {
                url: playlist_only_url(list_id),
                extra_args: vec!["--playlist-items".to_string(), index.to_string()],
            };
        }

        if video_id.is_some() {
            return VideoDownloadTarget {
                url: trimmed.to_string(),
                extra_args: vec!["--no-playlist".to_string()],
            };
        }

        return VideoDownloadTarget {
            url: playlist_only_url(list_id),
            extra_args: vec!["--playlist-items".to_string(), "1".to_string()],
        };
    }

    VideoDownloadTarget {
        url: trimmed.to_string(),
        extra_args: Vec::new(),
    }
}

pub fn resolve_yfsp_download(url: &str) -> Result<VideoDownloadTarget, String> {
    let trimmed = url.trim();
    if is_yfsp_master_playlist_url(trimmed) {
        return Err(
            "这个 MasterPlayList 是预览清单，不是正片；请粘贴 www.yfsp.tv/play/... 播放页链接"
                .to_string(),
        );
    }
    let normalized =
        normalize_yfsp_play_url(trimmed).ok_or_else(|| "不是有效的一帆视频播放链接".to_string())?;

    Ok(VideoDownloadTarget {
        url: normalized,
        extra_args: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_youtube_urls() {
        assert_eq!(
            detect_platform("https://www.youtube.com/watch?v=abc"),
            Some(VideoPlatform::Youtube)
        );
    }

    #[test]
    fn detects_twitter_urls() {
        assert_eq!(
            detect_platform("https://x.com/user/status/123"),
            Some(VideoPlatform::Twitter)
        );
    }

    #[test]
    fn detects_yfsp_play_urls_only_on_yfsp_hosts() {
        assert_eq!(
            detect_platform("https://www.yfsp.tv/play/WIK0XzFISyr?id=pRHRYXpZJC8"),
            Some(VideoPlatform::Yfsp)
        );
        assert_eq!(
            detect_platform("https://m.yfsp.tv/play/WIK0XzFISyr?id=pRHRYXpZJC8"),
            Some(VideoPlatform::Yfsp)
        );
        assert_eq!(
            detect_platform("https://yfsp.tv.evil.example/play/test?id=123"),
            Some(VideoPlatform::Generic)
        );
        assert_eq!(
            detect_platform("https://upload.yfsp.tv/api/video/MasterPlayList?id=123"),
            Some(VideoPlatform::Yfsp)
        );
    }

    #[test]
    fn keeps_yfsp_play_url_for_the_custom_extractor() {
        let original = "https://www.yfsp.tv/play/WIK0XzFISyr?id=pRHRYXpZJC8";
        let target = resolve_yfsp_download(original).unwrap();

        assert_eq!(target.url, original);
        assert!(target.extra_args.is_empty());
    }

    #[test]
    fn accepts_yfsp_movie_url_without_episode_id() {
        assert!(resolve_yfsp_download("https://www.yfsp.tv/play/WIK0XzFISyr").is_ok());
    }

    #[test]
    fn strips_ui_text_accidentally_appended_to_yfsp_video_id() {
        let target = resolve_yfsp_download("https://www.yfsp.tv/play/qdUF8COZEL4删除").unwrap();

        assert_eq!(target.url, "https://www.yfsp.tv/play/qdUF8COZEL4");
    }

    #[test]
    fn rejects_preview_master_playlist_url() {
        let original = "https://upload.yfsp.tv/api/video/MasterPlayList?id=bS77zt7A7WU";
        let error = resolve_yfsp_download(original).unwrap_err();

        assert!(error.contains("预览清单"));
    }

    #[test]
    fn accepts_generic_https_urls() {
        assert_eq!(
            detect_platform("https://www.bilibili.com/video/BV1xx411c7mD"),
            Some(VideoPlatform::Generic)
        );
        assert!(validate_video_url("https://example.com/watch?v=1").is_ok());
    }

    #[test]
    fn rejects_non_http_input() {
        assert!(validate_video_url("").is_err());
        assert!(validate_video_url("not-a-url").is_err());
        assert!(validate_video_url("yt-dlp failed: is not a valid URL").is_err());
    }
}
