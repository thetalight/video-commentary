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
    / 'yfsp.py'
)
SPEC = importlib.util.spec_from_file_location('yfsp_extractor', MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class SelectSourcesTests(unittest.TestCase):
    def test_extracts_nested_movie_title_and_rejects_site_boilerplate(self):
        response = {
            'title': '爱壹帆国际版-海量高清视频免费在线观看',
            'data': {'movie': {'movieName': '入殓师'}},
        }
        self.assertEqual(MODULE.YfspIE._metadata_title(response), '入殓师')
        self.assertIsNone(
            MODULE.YfspIE._clean_title('爱壹帆国际版-海量高清视频免费在线观看')
        )

    def test_prefers_full_clarity_stream_over_short_flv_fallback(self):
        info = {
            'clarity': [
                {
                    'memo': '1080p',
                    'path': {'result': 'https://media.example/full.m3u8'},
                }
            ],
            'flvPathList': [
                {'memo': 'preview', 'result': 'https://media.example/preview.mp4'}
            ],
        }

        self.assertEqual(
            MODULE.YfspIE._select_sources(info),
            [
                {
                    'result': 'https://media.example/full.m3u8',
                    'memo': '1080p',
                    'isHls': True,
                }
            ],
        )

    def test_uses_flv_paths_when_clarity_has_no_playable_stream(self):
        info = {
            'clarity': [{'memo': 'broken', 'path': {}}],
            'flvPathList': [
                {'memo': 'legacy', 'result': 'https://media.example/video.mp4'}
            ],
        }

        self.assertEqual(
            MODULE.YfspIE._select_sources(info),
            [
                {'memo': 'legacy', 'result': 'https://media.example/video.mp4'}
            ],
        )


class ProbeTests(unittest.TestCase):
    def test_clarity_labels_are_not_mistaken_for_the_movie_title(self):
        info = {
            'playingMedia': {'key': 'k', 'title': '576'},
            'clarity': [
                {'title': '1080', 'description': '蓝光'},
                {'title': '576', 'description': '标清'},
            ],
        }

        reject = MODULE.YfspIE._quality_labels(info)

        self.assertIn('576', reject)
        self.assertIn('蓝光', reject)
        # 没有片名字段时宁可用页面标题，也不能把画质标签当片名
        self.assertIsNone(MODULE.YfspIE._metadata_title(info, reject=reject))

    def test_subtitle_like_paths_finds_caption_fields_anywhere_in_the_response(self):
        response = {
            'data': {
                'info': {
                    'movieName': 'x',
                    'subtitlePath': 'https://cdn.example/a.srt',
                    'clarity': [{'memo': '1080p', 'caption': 'https://cdn.example/b.vtt'}],
                },
            },
        }

        self.assertEqual(
            MODULE.YfspIE._subtitle_like_paths(response),
            ['data.info.subtitlePath', 'data.info.clarity[0].caption'],
        )

    def test_subtitle_like_paths_is_empty_when_the_api_has_none(self):
        self.assertEqual(
            MODULE.YfspIE._subtitle_like_paths({'data': {'info': {'movieName': 'x'}}}),
            [],
        )


    def test_hint_strings_find_hidden_subtitle_endpoints_in_player_scripts(self):
        script = '''
        var cfg = { subtitleApi: "/v3/subtitle/list?key=" + key };
        fetch('/api/v3/video/captionList');
        player.loadSub("https://cdn.example/sub/abc.srt");
        var nothing = "/v3/video/play";
        '''

        hits = MODULE.YfspIE._hint_strings(script)

        self.assertTrue(any('subtitle' in hit for hit in hits), hits)
        self.assertTrue(any('captionList' in hit for hit in hits), hits)
        self.assertTrue(any(hit.endswith('abc.srt') for hit in hits), hits)


if __name__ == '__main__':
    unittest.main()
