import hashlib
import json
import re
import urllib.parse

from yt_dlp.extractor.common import InfoExtractor
from yt_dlp.utils import ExtractorError


class YfspIE(InfoExtractor):
    IE_NAME = 'yfsp'
    _VALID_URL = r'https?://(?:(?:www|m)\.)?yfsp\.tv/play/(?P<id>[^/?#]+)'
    _BASE_URL = 'https://www.yfsp.tv'
    _PLAY_API = 'https://m10.yfsp.tv/v3/video/play'
    _UA = (
        'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) '
        'AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36'
    )

    def _verification_error(self):
        raise ExtractorError(
            'YFSP verification required: open this page in Chrome, complete the '
            'Cloudflare check, and select Chrome cookies in the app settings',
            expected=True,
        )

    def _download_text(self, url, video_id, note, headers):
        text = self._download_webpage(url, video_id, note=note, headers=headers)
        if 'Just a moment...' in text or '__cf_chl_' in text:
            self._verification_error()
        return text

    @staticmethod
    def _signed_url(base_url, params, public_key, private_key):
        query = urllib.parse.urlencode(params)
        signature_input = f'{public_key}&{query.lower()}&{private_key}'
        signature = hashlib.md5(signature_input.encode()).hexdigest()
        return f'{base_url}?{query}&vv={signature}&pub={urllib.parse.quote(public_key)}'

    @staticmethod
    def _sign_stream_url(stream_url, public_key, private_key):
        parsed = urllib.parse.urlsplit(stream_url)
        query = parsed.query
        if not query or re.search(r'(?:^|&)vv=', query):
            return stream_url
        signature_input = f'{public_key}&{query.lower()}&{private_key}'
        signature = hashlib.md5(signature_input.encode()).hexdigest()
        signed_query = f'{query}&vv={signature}&pub={urllib.parse.quote(public_key)}'
        return urllib.parse.urlunsplit(parsed._replace(query=signed_query))

    @staticmethod
    def _first_dict(value):
        return value[0] if isinstance(value, list) and value and isinstance(value[0], dict) else {}

    @staticmethod
    def _clean_title(value):
        if not isinstance(value, str):
            return None
        value = re.split(r'\s*[-–—|_]\s*(?:免费在线观看|爱壹帆)', value, maxsplit=1)[0].strip()
        if not value or re.search(r'(?:爱壹帆国际版|海量高清视频免费在线观看)', value):
            return None
        return value

    @classmethod
    def _metadata_title(cls, *sources):
        preferred = {
            'videoname', 'vodname', 'moviename', 'filmname', 'showname',
            'albumname', 'seriesname', 'title', 'name',
        }
        for source in sources:
            queue = [source]
            while queue:
                value = queue.pop(0)
                if isinstance(value, dict):
                    for key, item in value.items():
                        if str(key).lower() in preferred:
                            cleaned = cls._clean_title(item)
                            if cleaned:
                                return cleaned
                    queue.extend(value.values())
                elif isinstance(value, list):
                    queue.extend(value)
        return None

    @staticmethod
    def _select_sources(info):
        """Prefer the same clarity streams used by the web player.

        YFSP may also return flvPathList entries that are short previews or
        interstitial clips.  Treat those as a legacy fallback only; otherwise
        yt-dlp can successfully download a playable but incorrect 20-second
        video instead of the full episode.
        """
        clarity = info.get('clarity') if isinstance(info.get('clarity'), list) else []
        clarity_sources = [
            {'result': item['path']['result'], 'memo': item.get('memo'), 'isHls': True}
            for item in clarity
            if isinstance(item, dict)
            and isinstance(item.get('path'), dict)
            and item['path'].get('result')
        ]
        if clarity_sources:
            return clarity_sources

        flv_paths = info.get('flvPathList') if isinstance(info.get('flvPathList'), list) else []
        return [item for item in flv_paths if isinstance(item, dict) and item.get('result')]

    @staticmethod
    def _subtitle_ext(url):
        path = urllib.parse.urlsplit(url).path.lower()
        ext = path.rsplit('.', 1)[-1] if '.' in path else 'vtt'
        return ext if ext in ('srt', 'vtt', 'ass', 'ssa', 'ttml', 'dfxp') else 'vtt'

    @classmethod
    def _api_subtitles(cls, info, headers):
        """Collect sidecar captions without depending on one unstable API key.

        YFSP has used several subtitle/caption field names across deployments.
        Only URLs found below a subtitle-like key are accepted, so ordinary
        poster, ad and stream URLs cannot be mistaken for captions.
        """
        subtitles = {}
        seen = set()

        def add(url, language):
            if not isinstance(url, str) or not url.startswith(('http://', 'https://')):
                return
            if url in seen:
                return
            seen.add(url)
            lang = language if isinstance(language, str) and language.strip() else 'zh'
            subtitles.setdefault(lang, []).append({
                'url': url,
                'ext': cls._subtitle_ext(url),
                'http_headers': headers,
            })

        def walk(value, language='zh', subtitle_context=False):
            if isinstance(value, dict):
                detected_language = next((
                    value.get(key) for key in ('lang', 'language', 'locale', 'srclang')
                    if isinstance(value.get(key), str) and value.get(key).strip()
                ), language)
                for key, item in value.items():
                    key_is_subtitle = bool(re.search(r'(?:sub(?:title)?|caption)', str(key), re.I))
                    child_context = subtitle_context or key_is_subtitle
                    if child_context and isinstance(item, str):
                        add(item, detected_language)
                    else:
                        walk(item, detected_language, child_context)
            elif isinstance(value, list):
                for item in value:
                    walk(item, language, subtitle_context)

        walk(info)
        return subtitles

    @staticmethod
    def _merge_subtitle_maps(target, source):
        for language, entries in (source or {}).items():
            if not isinstance(entries, list):
                continue
            existing = target.setdefault(language, [])
            existing_urls = {item.get('url') for item in existing if isinstance(item, dict)}
            existing.extend(
                item for item in entries
                if isinstance(item, dict) and item.get('url') not in existing_urls
            )

    def _real_extract(self, url):
        movie_id = self._match_id(url)
        episode_id = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query).get('id', [None])[0]
        media_id = episode_id or movie_id
        user_agent = (
            (self._downloader.params.get('http_headers') or {}).get('User-Agent')
            or self._UA
        )

        page_headers = {
            'Accept': 'text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8',
            'Accept-Language': 'zh-CN,zh;q=0.9,en;q=0.8',
            'User-Agent': user_agent,
        }
        webpage = self._download_text(url, media_id, 'Reading YFSP player configuration', page_headers)
        page_title = self._clean_title(self._html_extract_title(webpage, default=None))

        public_key = self._search_regex(
            r'"pConfig"\s*:\s*\{\s*"publicKey"\s*:\s*"([^"]+)"',
            webpage,
            'public signing key',
        )
        private_key = self._search_regex(
            r'"privateKey"\s*:\s*\[\s*"([^"]+)"',
            webpage,
            'private signing key',
        )

        params = [
            ('cinema', '1'),
            ('id', media_id),
            ('a', '0' if episode_id else '1'),
            ('usersign', '1'),
            ('region', 'GL.'),
            ('device', '1'),
            ('isMasterSupport', '1'),
        ]
        api_url = self._signed_url(self._PLAY_API, params, public_key, private_key)
        api_headers = {
            'Accept': 'application/json, text/plain, */*',
            'Referer': url,
            'User-Agent': user_agent,
            'X-Requested-With': 'XMLHttpRequest',
        }
        api_text = self._download_text(api_url, media_id, 'Resolving YFSP full video', api_headers)
        try:
            response = json.loads(api_text)
        except json.JSONDecodeError as error:
            raise ExtractorError(f'YFSP play API returned invalid JSON: {error}', expected=True)

        info = self._first_dict((response.get('data') or {}).get('info'))
        sources = self._select_sources(info)

        stream_headers = {
            'Origin': self._BASE_URL,
            'Referer': f'{self._BASE_URL}/',
            'User-Agent': user_agent,
        }
        formats = []
        subtitles = self._api_subtitles(info, stream_headers)
        seen = set()
        for index, source in enumerate(sources):
            stream_url = source.get('result')
            if not isinstance(stream_url, str) or stream_url in seen:
                continue
            seen.add(stream_url)
            stream_url = self._sign_stream_url(stream_url, public_key, private_key)
            memo = source.get('memo') or f'hls-{index + 1}'
            is_hls = bool(source.get('isHls')) or '.m3u8' in stream_url.lower()
            if is_hls:
                extracted, manifest_subtitles = self._extract_m3u8_formats_and_subtitles(
                    stream_url,
                    media_id,
                    'mp4',
                    m3u8_id=str(memo),
                    fatal=False,
                    headers=stream_headers,
                )
                formats.extend(extracted)
                self._merge_subtitle_maps(subtitles, manifest_subtitles)
            else:
                formats.append({
                    'format_id': str(memo),
                    'url': stream_url,
                    'http_headers': stream_headers,
                })

        if not formats:
            code = (response.get('data') or {}).get('code')
            raise ExtractorError(
                f'YFSP full video API returned no playable streams (code={code})',
                expected=True,
            )

        title = self._metadata_title(info, response) or page_title or media_id
        return {
            'id': media_id,
            'display_id': movie_id,
            'title': title,
            'formats': formats,
            'subtitles': subtitles,
            'http_headers': stream_headers,
        }
