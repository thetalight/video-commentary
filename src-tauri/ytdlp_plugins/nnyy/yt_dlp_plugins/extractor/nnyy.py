"""努努影院（nnyy.in）等苹果CMS(MacCMS)站的播放页解析。

这类站的播放页把播放地址放在 `<script>var player_aaaa = {...}</script>` 里：
`url` 是 m3u8/mp4 直链，或者是一个第三方线路页地址（from=hn/gs/md…），
`encrypt` 不为 0 时 `url` 被站点加密过。

只做「找到一条可下载的媒体地址」这一件事，下载、分片合并、字幕探测都交给 yt-dlp。
"""

import base64
import json
import re
from urllib.parse import parse_qs, unquote, urljoin, urlsplit

from yt_dlp.extractor.common import InfoExtractor
from yt_dlp.utils import ExtractorError

_MEDIA_URL_RE = re.compile(
    r'https?://[^\s"\'<>\\]+?\.(?:m3u8|mp4)(?:\?[^\s"\'<>\\]*)?', re.I)
_PLAYER_RE = re.compile(
    r'var\s+player_[A-Za-z0-9_]*\s*=\s*(\{.*?\})\s*(?:;|</script>)', re.S)
_EPISODE_RE = re.compile(r'href=["\']([^"\']+?-\d+-\d+\.html)["\']', re.I)
_TITLE_RE = re.compile(r'<title>\s*(.*?)\s*</title>', re.S | re.I)


class NnyyIE(InfoExtractor):
    IE_NAME = 'nnyy'
    _VALID_URL = (
        r'https?://(?:www\.)?nnyy\.in/'
        r'(?P<section>dianshiju|dianying|dongman|zongyi|jilupian)/'
        r'(?P<id>\d+)(?:-(?P<sid>\d+)-(?P<nid>\d+))?\.html'
    )
    _BASE = 'https://nnyy.in/'
    _HEADERS = {'Referer': 'https://nnyy.in/'}

    def _real_extract(self, url):
        match = self._match_valid_url(url)
        episode = match.groupdict()
        video_id = episode['id']
        if episode.get('nid'):
            video_id = f"{episode['id']}-{episode['sid']}-{episode['nid']}"

        webpage = self._download_webpage(
            url, video_id, headers=self._HEADERS, note='正在读取播放页')

        config = self._player_config(webpage)
        if not config:
            episodes = self._episode_links(webpage)
            if episodes:
                raise ExtractorError(
                    '这个链接是剧集列表页（共 %d 集），不是某一集的播放页；'
                    '请粘贴具体一集的地址，例如 %s' % (len(episodes), episodes[0]),
                    expected=True)
            raise ExtractorError(
                '播放页里没有找到播放器配置（站点模板可能变了，或这一页需要浏览器 JS）',
                expected=True)

        media_url, headers = self._resolve_media(config, url, video_id)
        title = self._page_title(webpage) or video_id

        if self._is_m3u8(media_url):
            formats, subtitles = self._extract_m3u8_formats_and_subtitles(
                media_url, video_id, 'mp4', headers=headers, fatal=False)
            if not formats:
                raise ExtractorError('m3u8 清单里没有可下载的清晰度', expected=True)
        else:
            formats, subtitles = [{
                'url': media_url,
                'format_id': 'direct',
                'ext': 'mp4',
            }], {}

        return {
            'id': video_id,
            'title': title,
            'formats': formats,
            'subtitles': subtitles,
            'http_headers': headers,
        }

    # ---- 纯解析部分：不联网，便于单元测试 -------------------------------

    @staticmethod
    def _loads(raw):
        for candidate in (raw, raw.replace("'", '"')):
            try:
                return json.loads(candidate)
            except (TypeError, ValueError):
                continue
        return None

    @classmethod
    def _player_config(cls, webpage):
        for raw in _PLAYER_RE.findall(webpage or ''):
            data = cls._loads(raw)
            if isinstance(data, dict) and (data.get('url') or data.get('link')):
                return data
        return None

    @classmethod
    def _episode_links(cls, webpage):
        found = []
        for href in _EPISODE_RE.findall(webpage or ''):
            absolute = urljoin(cls._BASE, href)
            if absolute not in found:
                found.append(absolute)
        return found

    @staticmethod
    def _decode_play_url(raw, encrypt):
        """encrypt != 0 时站点会把地址做一次 URL 编码 + base64。

        不同模板细节不一，这里把常见的几种都试一遍；都失败就原样返回，
        由调用方给出「被加密」的明确错误，而不是静默失败。
        """
        raw = (raw or '').strip()
        try:
            mode = int(encrypt or 0)
        except (TypeError, ValueError):
            mode = 0
        if mode == 0 or not raw:
            return raw

        candidates = []
        for text in (raw, unquote(raw)):
            padded = text + '=' * (-len(text) % 4)
            for decoder in (base64.urlsafe_b64decode, base64.b64decode):
                try:
                    decoded = decoder(padded).decode('utf-8', 'ignore').strip()
                except Exception:
                    continue
                if decoded:
                    candidates.extend([decoded, unquote(decoded)])
        for candidate in candidates:
            if re.match(r'(?:https?://|/)', candidate, re.I):
                return candidate
        return raw

    @classmethod
    def _media_from_candidate(cls, candidate):
        text = (candidate or '').strip()
        if not text:
            return None
        # 线路页常把真实地址塞进 ?url=，而且这些页面地址本身也以 .m3u8 结尾，
        # 所以必须先看查询参数，再判断整条地址是不是媒体直链。
        query = parse_qs(urlsplit(text).query)
        for key in ('url', 'v', 'src', 'video'):
            for value in query.get(key, []):
                nested = unquote(value)
                if _MEDIA_URL_RE.fullmatch(nested):
                    return nested
        if _MEDIA_URL_RE.fullmatch(text):
            return text
        return None

    @classmethod
    def _first_media_url(cls, webpage):
        match = _MEDIA_URL_RE.search(webpage or '')
        return match.group(0) if match else None

    @staticmethod
    def _is_m3u8(url):
        try:
            return urlsplit(url).path.lower().endswith('.m3u8')
        except ValueError:
            return False

    @staticmethod
    def _page_title(webpage):
        match = _TITLE_RE.search(webpage or '')
        if not match:
            return None
        title = re.sub(r'\s+', ' ', match.group(1)).strip()
        title = re.split(r'\s*[-_|]\s*(?:努努|nnyy)', title, flags=re.I)[0].strip()
        return title or None

    # ---- 需要联网的兜底：第三方线路页 -----------------------------------

    def _resolve_media(self, config, page_url, video_id):
        raw = (config.get('url') or '').strip()
        if not raw:
            raise ExtractorError('播放器配置里没有播放地址', expected=True)

        candidate = self._decode_play_url(raw, config.get('encrypt'))
        if candidate.startswith('/'):
            candidate = urljoin(page_url, candidate)

        direct = self._media_from_candidate(candidate)
        if direct:
            return direct, {'Referer': page_url}

        if not re.match(r'https?://', candidate, re.I):
            raise ExtractorError(
                '播放地址被站点加密（encrypt=%s），暂时无法解密：%s'
                % (config.get('encrypt'), raw), expected=True)

        # 采集站的多线路：from=hn/gs/md… 往往是一个独立播放页，再抓一层。
        page = self._download_webpage(
            candidate, video_id, headers=self._HEADERS, note='正在解析播放线路')
        inner = self._player_config(page)
        if inner:
            nested = self._decode_play_url(inner.get('url') or '', inner.get('encrypt'))
            if nested.startswith('/'):
                nested = urljoin(candidate, nested)
            media = self._media_from_candidate(nested)
            if media:
                return media, {'Referer': candidate}
        found = self._first_media_url(page)
        if found:
            return found, {'Referer': candidate}
        raise ExtractorError('线路页里没有找到 m3u8/mp4 地址', expected=True)
