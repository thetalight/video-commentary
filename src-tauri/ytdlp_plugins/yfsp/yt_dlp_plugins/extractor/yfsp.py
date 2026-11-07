import hashlib
import json
import os
import re
import socket
import subprocess
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path

from yt_dlp.extractor.common import InfoExtractor
from yt_dlp.utils import ExtractorError


class YfspIE(InfoExtractor):
    IE_NAME = 'yfsp'
    _VALID_URL = r'https?://(?:(?:www|m)\.)?yfsp\.tv/play/(?P<id>[^/?#]+)'
    _BASE_URL = 'https://www.yfsp.tv'
    _PLAY_API = 'https://m10.yfsp.tv/v3/video/play'
    _CHROME_PATH = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
    _CHROME_WAIT_SECONDS = 180
    _UA = (
        'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) '
        'AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36'
    )

    def _verification_error(self):
        raise ExtractorError(
            'YFSP verification required: complete the Cloudflare check in the '
            'dedicated Chrome window opened by the app',
            expected=True,
        )

    def _download_text(
        self,
        url,
        video_id,
        note,
        headers,
        impersonate=None,
        browser_fallback=False,
    ):
        text = None
        for attempt in range(4):
            try:
                text = self._download_webpage(
                    url,
                    video_id,
                    note=note if attempt == 0 else f'{note} (retry {attempt + 1}/4)',
                    headers=headers,
                    impersonate=impersonate,
                    require_impersonation=bool(impersonate),
                )
                break
            except ExtractorError as error:
                retryable = re.search(
                    r'(?:SSL_connect|SSL_ERROR_SYSCALL|connection closed|connection reset|'
                    r'remote end closed|timed? out|TransportError|UNEXPECTED_EOF|'
                    r'EOF occurred in violation of protocol)',
                    str(error),
                    re.I,
                )
                if retryable and attempt == 3 and browser_fallback:
                    text = self._chrome_fetch_text(url, video_id, headers)
                    break
                if not retryable or attempt == 3:
                    raise
                time.sleep(2 ** attempt)
        if 'Just a moment...' in text or '__cf_chl_' in text:
            self._verification_error()
        return text

    @staticmethod
    def _local_json(url):
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with opener.open(url, timeout=2) as response:
            return json.load(response)

    @staticmethod
    def _free_local_port():
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.bind(('127.0.0.1', 0))
            return sock.getsockname()[1]

    def _has_clearance_cookie(self):
        return 'cf_clearance' in self._get_cookies(self._BASE_URL)

    @staticmethod
    def _chrome_profile_dir():
        return Path(os.environ.get(
            'YFSP_CHROME_PROFILE',
            Path.home() / 'Library/Application Support/com.esun.video-commentary/yfsp-chrome-profile',
        ))

    @staticmethod
    def _existing_chrome_cdp_port(profile_dir):
        """Find Chrome already owning this profile and its debugging port."""
        try:
            output = subprocess.run(
                ['/bin/ps', '-axo', 'command='],
                capture_output=True,
                text=True,
                timeout=2,
                check=False,
            ).stdout
        except (OSError, subprocess.SubprocessError):
            return None

        profile = str(profile_dir)
        for line in output.splitlines():
            if profile not in line:
                continue
            match = re.search(r'--remote-debugging-port=(\d+)', line)
            if not match:
                continue
            port = int(match.group(1))
            try:
                version = YfspIE._local_json(f'http://127.0.0.1:{port}/json/version')
            except Exception:
                continue
            if version.get('webSocketDebuggerUrl'):
                return port
        return None

    @classmethod
    def _start_chrome_session(cls, url, profile_dir):
        existing_port = cls._existing_chrome_cdp_port(profile_dir)
        if existing_port is not None:
            return existing_port, None, False

        port = cls._free_local_port()
        process = subprocess.Popen(
            [
                cls._CHROME_PATH,
                '--no-first-run',
                '--no-default-browser-check',
                f'--remote-debugging-port={port}',
                f'--user-data-dir={profile_dir}',
                url,
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        return port, process, True

    @classmethod
    def _ensure_chrome_page(cls, port, url):
        """Create a page when an existing profile has no inspectable tab."""
        try:
            tabs = cls._local_json(f'http://127.0.0.1:{port}/json/list')
        except Exception:
            tabs = []
        if any('yfsp.tv' in (tab.get('url') or '') for tab in tabs):
            return
        request = urllib.request.Request(
            f'http://127.0.0.1:{port}/json/new?{urllib.parse.quote(url, safe="")}',
            method='PUT',
        )
        try:
            with urllib.request.urlopen(request, timeout=5):
                pass
        except Exception:
            # The normal launch path will still be able to report a useful
            # timeout if Chrome refuses to create a target.
            pass

    def _import_cdp_cookies(self, cookies):
        imported = 0
        for cookie in cookies:
            domain = cookie.get('domain') or ''
            name = cookie.get('name')
            value = cookie.get('value')
            if 'yfsp.tv' not in domain or not name or value is None:
                continue
            expires = cookie.get('expires')
            self._set_cookie(
                domain,
                name,
                value,
                expire_time=int(expires) if expires and expires > 0 else None,
                path=cookie.get('path') or '/',
                secure=bool(cookie.get('secure')),
                discard=not bool(expires and expires > 0),
                rest={'HttpOnly': None} if cookie.get('httpOnly') else {},
            )
            imported += 1
        return imported

    def _chrome_session(self, url, video_id, fallback_user_agent):
        """Let Chrome solve Cloudflare and return decrypted YFSP cookies via CDP.

        Recent Chromium releases on macOS may keep their cookie encryption key
        in Apple Passwords/iCloud Keychain.  yt-dlp's --cookies-from-browser
        cannot read that key, while Chrome itself can.  A dedicated persistent
        profile plus the local DevTools protocol avoids exporting or decrypting
        the user's normal browser database.
        """
        if self._has_clearance_cookie() or sys.platform != 'darwin':
            return fallback_user_agent
        if not os.path.isfile(self._CHROME_PATH):
            return fallback_user_agent

        try:
            from websockets.sync.client import connect
        except ImportError:
            self.report_warning('Chrome cookie bridge unavailable: Python websockets is missing')
            return fallback_user_agent

        profile_dir = self._chrome_profile_dir()
        profile_dir.mkdir(parents=True, exist_ok=True)
        port, process, owns_process = self._start_chrome_session(url, profile_dir)

        browser_socket = None
        imported = 0
        user_agent = fallback_user_agent
        deadline = time.monotonic() + self._CHROME_WAIT_SECONDS
        self.to_screen(
            f'{video_id}: Chrome verification window opened; complete the check if prompted')
        try:
            version = None
            while time.monotonic() < deadline:
                try:
                    version = self._local_json(f'http://127.0.0.1:{port}/json/version')
                    break
                except Exception:
                    if owns_process and process.poll() is not None:
                        break
                    time.sleep(0.25)
            if not version or not version.get('webSocketDebuggerUrl'):
                raise ExtractorError('Unable to start the YFSP Chrome verification session', expected=True)

            user_agent = version.get('User-Agent') or fallback_user_agent
            self._ensure_chrome_page(port, url)
            browser_socket = connect(version['webSocketDebuggerUrl'], open_timeout=3)
            request_id = 0
            last_notice = 0
            while time.monotonic() < deadline:
                request_id += 1
                browser_socket.send(json.dumps({'id': request_id, 'method': 'Storage.getCookies'}))
                while True:
                    message = json.loads(browser_socket.recv(timeout=3))
                    if message.get('id') == request_id:
                        break
                cookies = message.get('result', {}).get('cookies', [])
                has_clearance = any(
                    cookie.get('name') == 'cf_clearance'
                    and 'yfsp.tv' in (cookie.get('domain') or '')
                    for cookie in cookies
                )
                try:
                    tabs = self._local_json(f'http://127.0.0.1:{port}/json/list')
                except Exception:
                    tabs = []
                page_ready = any(
                    'yfsp.tv' in (tab.get('url') or '')
                    and tab.get('title')
                    and not re.search(
                        r'(?:Just a moment|请稍候|404|服务器错误)',
                        tab['title'],
                        re.I,
                    )
                    for tab in tabs
                )
                if has_clearance and page_ready:
                    imported = self._import_cdp_cookies(cookies)
                    break
                elapsed = int(self._CHROME_WAIT_SECONDS - (deadline - time.monotonic()))
                if elapsed - last_notice >= 15:
                    self.to_screen(f'{video_id}: Waiting for Chrome verification...')
                    last_notice = elapsed
                time.sleep(0.5)
        finally:
            if browser_socket is not None:
                if owns_process:
                    try:
                        request_id += 1
                        browser_socket.send(json.dumps({'id': request_id, 'method': 'Browser.close'}))
                    except Exception:
                        pass
                try:
                    browser_socket.close()
                except Exception:
                    pass
            if owns_process:
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.terminate()

        if not imported or not self._has_clearance_cookie():
            raise ExtractorError(
                'YFSP Chrome verification timed out; complete the check in the opened Chrome window',
                expected=True,
            )
        self.to_screen(f'{video_id}: Chrome verification cookies imported')
        return user_agent

    def _chrome_fetch_text(self, url, video_id, headers):
        """Fetch a YFSP API URL inside the verified Chrome page context.

        Some proxy nodes abruptly close TLS handshakes made by curl_cffi or
        Python/OpenSSL while allowing Chrome itself.  This is only a fallback
        after four ordinary request attempts fail.
        """
        if sys.platform != 'darwin' or not os.path.isfile(self._CHROME_PATH):
            raise ExtractorError('YFSP Chrome API fallback is unavailable', expected=True)
        try:
            from websockets.sync.client import connect
        except ImportError as error:
            raise ExtractorError(
                'YFSP Chrome API fallback requires Python websockets',
                expected=True,
            ) from error

        profile_dir = self._chrome_profile_dir()
        profile_dir.mkdir(parents=True, exist_ok=True)
        referer = headers.get('Referer') or self._BASE_URL
        port, process, owns_process = self._start_chrome_session(referer, profile_dir)

        browser_socket = None
        page_socket = None
        deadline = time.monotonic() + 90
        self.to_screen(f'{video_id}: Retrying YFSP API inside Chrome')
        try:
            version = None
            while time.monotonic() < deadline:
                try:
                    version = self._local_json(f'http://127.0.0.1:{port}/json/version')
                    break
                except Exception:
                    if owns_process and process.poll() is not None:
                        break
                    time.sleep(0.25)
            if not version or not version.get('webSocketDebuggerUrl'):
                raise ExtractorError('Unable to start Chrome for the YFSP API fallback', expected=True)

            self._ensure_chrome_page(port, referer)
            browser_socket = connect(version['webSocketDebuggerUrl'], open_timeout=3)
            target = None
            while time.monotonic() < deadline:
                try:
                    tabs = self._local_json(f'http://127.0.0.1:{port}/json/list')
                except Exception:
                    tabs = []
                target = next((
                    tab for tab in tabs
                    if 'yfsp.tv' in (tab.get('url') or '')
                    and tab.get('webSocketDebuggerUrl')
                    and tab.get('title')
                    and not re.search(
                        r'(?:Just a moment|请稍候|404|服务器错误)',
                        tab['title'],
                        re.I,
                    )
                ), None)
                if target:
                    break
                time.sleep(0.5)
            if not target:
                raise ExtractorError('The verified YFSP page did not become ready in Chrome', expected=True)

            allowed_headers = {
                key: value for key, value in headers.items()
                if key.lower() in ('accept', 'x-requested-with')
            }
            fetch_options = {
                'credentials': 'include',
                'headers': allowed_headers,
            }
            expression = (
                '(async () => {'
                f'const response = await fetch({json.dumps(url)}, {json.dumps(fetch_options)});'
                'return JSON.stringify({status: response.status, text: await response.text()});'
                '})()'
            )
            page_socket = connect(target['webSocketDebuggerUrl'], open_timeout=3)
            page_socket.send(json.dumps({
                'id': 1,
                'method': 'Runtime.evaluate',
                'params': {
                    'expression': expression,
                    'awaitPromise': True,
                    'returnByValue': True,
                },
            }))
            while True:
                message = json.loads(page_socket.recv(timeout=30))
                if message.get('id') == 1:
                    break
            if message.get('result', {}).get('exceptionDetails'):
                raise ExtractorError('Chrome could not fetch the YFSP play API', expected=True)
            value = message.get('result', {}).get('result', {}).get('value')
            payload = json.loads(value) if isinstance(value, str) else {}
            status = int(payload.get('status') or 0)
            if status < 200 or status >= 300:
                raise ExtractorError(
                    f'YFSP play API returned HTTP {status} inside Chrome',
                    expected=True,
                )
            return payload.get('text') or ''
        finally:
            if page_socket is not None:
                try:
                    page_socket.close()
                except Exception:
                    pass
            if browser_socket is not None:
                if owns_process:
                    try:
                        browser_socket.send(json.dumps({'id': 99, 'method': 'Browser.close'}))
                    except Exception:
                        pass
                try:
                    browser_socket.close()
                except Exception:
                    pass
            if owns_process:
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.terminate()

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

    @staticmethod
    def _quality_labels(info):
        """clarity / playingMedia 里的 title 是画质标签（576、720、蓝光…），不是片名。"""
        labels = set()
        playing = info.get('playingMedia')
        if isinstance(playing, dict) and isinstance(playing.get('title'), str):
            labels.add(playing['title'].strip())
        clarity = info.get('clarity') if isinstance(info.get('clarity'), list) else []
        for item in clarity:
            if not isinstance(item, dict):
                continue
            for key in ('title', 'description', 'memo'):
                if isinstance(item.get(key), str):
                    labels.add(item[key].strip())
        return {label for label in labels if label}

    @classmethod
    def _metadata_title(cls, *sources, reject=None):
        reject = reject or set()
        preferred = {
            'videoname', 'vodname', 'moviename', 'filmname', 'showname',
            'mediatitle',
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
                            if cleaned and cleaned not in reject:
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
        return [
            item for item in flv_paths
            if isinstance(item, dict)
            and item.get('result')
            and (not isinstance(item.get('duration'), (int, float))
                 or item['duration'] <= 0 or item['duration'] >= 120)
        ]

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

    @staticmethod
    def _subtitle_like_paths(value, prefix='', found=None):
        """列出响应里所有「像字幕」的字段路径，便于排查平台到底有没有字幕。"""
        if found is None:
            found = []
        if len(found) >= 200:
            return found
        if isinstance(value, dict):
            for key, item in value.items():
                path = f'{prefix}.{key}' if prefix else str(key)
                if re.search(r'(?:sub(?:title)?|caption|srt|vtt|\.ass$)', str(key), re.I):
                    found.append(path)
                YfspIE._subtitle_like_paths(item, path, found)
        elif isinstance(value, list):
            for index, item in enumerate(value[:20]):
                YfspIE._subtitle_like_paths(item, f'{prefix}[{index}]', found)
        return found

    @staticmethod
    def _hint_strings(text, limit=40):
        """挑出文本里所有提到 subtitle/caption/srt/vtt 的片段（找隐藏的字幕接口用）。"""
        found = []
        if not text:
            return found
        pattern = re.compile(
            r'''[^\s"'<>]{0,160}(?:sub(?:title)?|caption|\.srt|\.vtt)[^\s"'<>]{0,160}''',
            re.I,
        )
        for match in pattern.finditer(text):
            value = match.group(0).strip()
            if len(value) < 4 or value in found:
                continue
            found.append(value)
            if len(found) >= limit:
                break
        return found

    def _player_probe(self, url, webpage, media_id, page_headers):
        """播放接口里没有字幕时，翻页面和播放器脚本，找它另外调用的字幕接口。"""
        probe = {
            'script_urls': [],
            'page_hits': self._hint_strings(webpage),
            'script_hits': {},
            'errors': [],
        }
        scripts = re.findall(r'<script[^>]+src=["\']([^"\']+)["\']', webpage or '', re.I)
        absolute = []
        for src in scripts:
            full = urllib.parse.urljoin(url, src)
            if full.startswith('http') and full not in absolute:
                absolute.append(full)
        probe['script_urls'] = absolute[:12]

        for full in absolute[:6]:
            try:
                text = self._download_text(
                    full,
                    media_id,
                    f'Probing player script {Path(full).name}',
                    {**page_headers, 'Referer': url},
                    impersonate='chrome',
                )
            except Exception as error:
                probe['errors'].append(f'{full}: {error}')
                continue
            hits = self._hint_strings(text)
            if hits:
                probe['script_hits'][full] = hits
        return probe

    def _write_text(self, name, text):
        try:
            directory = Path.home() / (
                'Library/Application Support/com.esun.video-commentary/debug')
            directory.mkdir(parents=True, exist_ok=True)
            (directory / name).write_text((text or '')[:1500000], encoding='utf-8')
        except Exception:
            pass

    def _write_probe(self, payload):
        """把播放接口的原始返回落盘，供「平台有没有字幕」这类问题定位。"""
        try:
            directory = Path.home() / (
                'Library/Application Support/com.esun.video-commentary/debug')
            directory.mkdir(parents=True, exist_ok=True)
            (directory / 'yfsp-last.json').write_text(
                json.dumps(payload, ensure_ascii=False, indent=2, default=str)[:2000000],
                encoding='utf-8',
            )
        except Exception:
            # 诊断信息永远不能影响正常解析
            pass

    def _real_extract(self, url):
        movie_id = self._match_id(url)
        episode_id = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query).get('id', [None])[0]
        media_id = episode_id or movie_id
        configured_user_agent = (
            (self._downloader.params.get('http_headers') or {}).get('User-Agent')
            or self._UA
        )
        user_agent = self._chrome_session(url, media_id, configured_user_agent)

        page_headers = {
            'Accept': 'text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8',
            'Accept-Language': 'zh-CN,zh;q=0.9,en;q=0.8',
            'User-Agent': user_agent,
        }
        webpage = self._download_text(
            url,
            media_id,
            'Reading YFSP player configuration',
            page_headers,
            impersonate='chrome',
        )
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
        api_text = self._download_text(
            api_url,
            media_id,
            'Resolving YFSP full video',
            api_headers,
            browser_fallback=True,
        )
        try:
            response = json.loads(api_text)
        except json.JSONDecodeError as error:
            raise ExtractorError(f'YFSP play API returned invalid JSON: {error}', expected=True)

        info = self._first_dict((response.get('data') or {}).get('info'))
        if str(info.get('isPreView', '')).lower() in ('true', '1') or str(
            info.get('isLimitedStream', '')).lower() in ('true', '1'):
            raise ExtractorError(
                '一帆播放接口只返回预览或受限片源，未找到正片；请检查播放页与播放线路',
                expected=True,
            )
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

        title = (
            self._metadata_title(info, response, reject=self._quality_labels(info))
            or page_title
            or media_id
        )
        player_probe = {}
        if not subtitles:
            # 平台接口没给字幕时，再翻一层播放器脚本，看是否有独立的字幕接口。
            try:
                player_probe = self._player_probe(url, webpage, media_id, page_headers)
                self._write_text('yfsp-page.html', webpage)
            except Exception as error:
                player_probe = {'fatal': str(error)}
        self._write_probe({
            'url': url,
            'fetched_at': time.strftime('%Y-%m-%d %H:%M:%S'),
            'api_url': api_url,
            'api_code': (response.get('data') or {}).get('code'),
            'page_title': page_title,
            'resolved_title': title,
            'api_response': response,
            'info_keys': sorted(info.keys()) if isinstance(info, dict) else None,
            'subtitle_like_paths': self._subtitle_like_paths(response),
            'computed_subtitles': subtitles,
            'player_probe': player_probe,
        })
        return {
            'id': media_id,
            'display_id': movie_id,
            'title': title,
            'formats': formats,
            'subtitles': subtitles,
            'http_headers': stream_headers,
        }
