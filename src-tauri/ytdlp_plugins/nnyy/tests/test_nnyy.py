import base64
import importlib.util
from pathlib import Path
import sys
import types
import unittest


class InfoExtractor:
    pass


class ExtractorError(Exception):
    pass


yt_dlp = types.ModuleType('yt_dlp')
yt_dlp_extractor = types.ModuleType('yt_dlp.extractor')
yt_dlp_common = types.ModuleType('yt_dlp.extractor.common')
yt_dlp_utils = types.ModuleType('yt_dlp.utils')
yt_dlp_common.InfoExtractor = InfoExtractor
yt_dlp_utils.ExtractorError = ExtractorError
sys.modules.setdefault('yt_dlp', yt_dlp)
sys.modules.setdefault('yt_dlp.extractor', yt_dlp_extractor)
sys.modules.setdefault('yt_dlp.extractor.common', yt_dlp_common)
sys.modules.setdefault('yt_dlp.utils', yt_dlp_utils)


MODULE_PATH = (
    Path(__file__).parents[1]
    / 'yt_dlp_plugins'
    / 'extractor'
    / 'nnyy.py'
)
SPEC = importlib.util.spec_from_file_location('nnyy_extractor', MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


PLAYER_PAGE = '''
<title>天道 第01集 - 努努影院</title>
<div class="player"></div>
<script type="text/javascript">
var player_aaaa={"flag":"play","encrypt":0,"trysee":0,"points":0,
"link":"\\/dianshiju\\/20080573-1-1.html","url":"https:\\/\\/cdn.example.com\\/hls\\/20080573\\/index.m3u8",
"from":"hn","server":"no","note":"","id":"20080573","sid":1,"nid":1}
</script>
'''

LIST_PAGE = '''
<title>天道 全集在线观看 - 努努影院</title>
<a href="/dianshiju/20080573-1-1.html">第01集</a>
<a href="/dianshiju/20080573-2-1.html">第01集</a>
<a href="/dianshiju/20080573-1-1.html">第01集</a>
'''


class PlayerConfigTests(unittest.TestCase):
    def test_reads_maccms_player_config_from_a_play_page(self):
        config = MODULE.NnyyIE._player_config(PLAYER_PAGE)

        self.assertEqual(config['from'], 'hn')
        self.assertEqual(config['encrypt'], 0)
        self.assertEqual(
            MODULE.NnyyIE._decode_play_url(config['url'], config['encrypt']),
            'https://cdn.example.com/hls/20080573/index.m3u8',
        )
        self.assertEqual(MODULE.NnyyIE._page_title(PLAYER_PAGE), '天道 第01集')

    def test_detail_pages_have_no_player_but_do_list_episodes(self):
        self.assertIsNone(MODULE.NnyyIE._player_config(LIST_PAGE))
        self.assertEqual(
            MODULE.NnyyIE._episode_links(LIST_PAGE),
            [
                'https://nnyy.in/dianshiju/20080573-1-1.html',
                'https://nnyy.in/dianshiju/20080573-2-1.html',
            ],
        )

    def test_single_quoted_player_config_still_parses(self):
        page = "<script>var player_aaaa={'url': 'https://cdn.example.com/x.m3u8', 'encrypt': 0}</script>"

        config = MODULE.NnyyIE._player_config(page)

        self.assertEqual(config['url'], 'https://cdn.example.com/x.m3u8')


class MediaResolutionTests(unittest.TestCase):
    def test_third_party_player_page_carries_the_media_url_in_a_query_param(self):
        candidate = (
            'https://play.example.com/index.php?url='
            'https%3A%2F%2Fcdn.example.com%2Fv%2Fa.m3u8'
        )

        self.assertEqual(
            MODULE.NnyyIE._media_from_candidate(candidate),
            'https://cdn.example.com/v/a.m3u8',
        )

    def test_base64_encrypted_play_url_is_decoded(self):
        encoded = base64.b64encode(b'https://cdn.example.com/a.m3u8').decode()

        self.assertEqual(
            MODULE.NnyyIE._decode_play_url(encoded, 2),
            'https://cdn.example.com/a.m3u8',
        )

    def test_undecryptable_play_url_is_returned_unchanged_for_a_clear_error(self):
        self.assertEqual(MODULE.NnyyIE._decode_play_url('not-encrypted-here', 2), 'not-encrypted-here')

    def test_plain_media_url_is_recognised(self):
        self.assertTrue(MODULE.NnyyIE._is_m3u8('https://cdn.example.com/a.m3u8?token=1'))
        self.assertFalse(MODULE.NnyyIE._is_m3u8('https://cdn.example.com/a.mp4'))
        self.assertEqual(
            MODULE.NnyyIE._first_media_url('<video src="https://cdn.example.com/a.mp4"></video>'),
            'https://cdn.example.com/a.mp4',
        )


if __name__ == '__main__':
    unittest.main()
