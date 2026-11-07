# Third-party notices

## Optional Qwen3-TTS / MLX local speech runtime

The independent offline adapter can use [Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS) CustomVoice weights (Apache-2.0) through [mlx-audio](https://github.com/Blaizzy/mlx-audio) (MIT). The optional installer pins mlx-audio 0.6.4 and the approved MLX checkpoint revision. Model weights and Python dependencies are installed separately in the user's application-data directory, not distributed in this repository. Preserve upstream licenses when redistributing those components; see docs/QWEN_TTS.md.

## Rig (rig-core 0.42.0)

The DeepSeek text-generation adapter depends on [Rig](https://github.com/0xPlaygrounds/rig), under the MIT License.

Copyright (c) 2024, Playgrounds Analytics Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## NarratoAI

Parts of the narration continuity and timing validation design are adapted from
[NarratoAI](https://github.com/linyqh/NarratoAI), Copyright (c) 2024 linyq.

NarratoAI is licensed under the MIT License:

> Permission is hereby granted, free of charge, to any person obtaining a copy
> of this software and associated documentation files (the "Software"), to deal
> in the Software without restriction, including without limitation the rights
> to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
> copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all
> copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
> IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
> FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
> AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
> LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
> SOFTWARE.

## Design references (no copied code)

- [movie-narrator](https://github.com/zcbacxc/movie-narrator) — multi-candidate review and local Ollama workflow. Licensed AGPL-3.0; no source code copied into this project.
- [Yapper](https://github.com/yuchia329/yapper) — staged, resumable pipeline and MAP/REDUCE story planning. No source code copied because the repository does not publish a clear reusable license.
