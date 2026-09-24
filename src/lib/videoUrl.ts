export type VideoPlatform = "youtube" | "twitter" | "yfsp" | "generic";

export function normalizeVideoUrl(url: string): string {
  const trimmed = url.trim();
  try {
    const parsed = new URL(trimmed);
    const host = parsed.hostname.toLowerCase();
    if (
      (host === "yfsp.tv" || host.endsWith(".yfsp.tv")) &&
      parsed.pathname.startsWith("/play/")
    ) {
      const rawId = parsed.pathname.slice("/play/".length);
      const videoId = rawId.match(/^[A-Za-z0-9_-]+/)?.[0];
      if (videoId) {
        parsed.pathname = `/play/${videoId}`;
        parsed.hash = "";
        return parsed.toString();
      }
    }
  } catch {
    // Validation reports malformed URLs to the user.
  }
  return trimmed;
}

export function detectPlatform(url: string): VideoPlatform | null {
  const lower = url.trim().toLowerCase();
  if (lower.includes("youtube.com") || lower.includes("youtu.be")) {
    return "youtube";
  }
  if (
    lower.includes("x.com/") ||
    lower.includes("twitter.com/") ||
    lower.includes("mobile.twitter.com/")
  ) {
    return "twitter";
  }
  try {
    const parsed = new URL(url.trim());
    const host = parsed.hostname.toLowerCase();
    if (
      (host === "yfsp.tv" || host.endsWith(".yfsp.tv")) &&
      parsed.pathname.startsWith("/play/")
    ) {
      return "yfsp";
    }
  } catch {
    // The existing validation below handles malformed URLs.
  }
  if (lower.startsWith("http://") || lower.startsWith("https://")) {
    return "generic";
  }
  return null;
}

export function isValidVideoUrl(url: string): boolean {
  const trimmed = normalizeVideoUrl(url);
  if (!trimmed.startsWith("http://") && !trimmed.startsWith("https://")) {
    return false;
  }
  if (
    trimmed.includes("yt-dlp failed") ||
    trimmed.includes("is not a valid URL")
  ) {
    return false;
  }
  return detectPlatform(trimmed) !== null;
}

export function videoUrlErrorMessage(): string {
  return "请输入以 https:// 开头的视频链接";
}
