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


if __name__ == '__main__':
    unittest.main()
