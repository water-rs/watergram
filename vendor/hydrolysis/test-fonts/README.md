# Deterministic test fonts

Roboto, from the release the `water` CLI's font registry ships to applications
(<https://github.com/googlefonts/roboto/releases/download/v2.138/roboto-android.zip>),
licensed under the Apache License 2.0 (see `LICENSE` beside the files).

`NotoSans*-Regular` faces, `pyftsubset` subsets of the Noto Sans families the
CLI ships to applications (`fonts-noto-cjk` and `fonts-noto-core` upstreams,
<https://github.com/googlefonts/noto-cjk> and
<https://github.com/googlefonts/noto-fonts>), kept to the sample strings the
suite shapes and licensed under the SIL Open Font License 1.1 (see
`LICENSE-OFL.txt` beside the files).

Headless test hosts shape text against these fonts instead of whatever the
host OS discovers, so a layout assertion tuned on one platform's system fonts
holds on every other, and snapshot goldens are identical across macOS, Linux,
and Windows runners. The Noto subsets play the fallback half of the same
guarantee: a script Roboto cannot cover resolves to a bundled Noto face rather
than to whatever a given runner happened to install — or did not, as a clean
CI image carries no CJK, Hangul, Thai, or Devanagari face at all.

Production rendering is untouched: applications keep the resource fonts the
CLI stages next to the executable, and the system fallback chain behind them.
