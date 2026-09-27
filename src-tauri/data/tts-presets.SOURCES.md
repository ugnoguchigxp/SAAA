# TTS preset sources

`tts-presets.tsv` contains 80,205 written-form/readings pairs. It is an intersection of the two sources below, not a copy of either complete lexicon. Only forms with exactly one reading in each source and the same reading in both are included. Sudachi proper nouns, forms without a kanji character, and forms with non-katakana readings are omitted. User dictionary entries override these presets.

| Source | Version | Archive | SHA-256 |
| --- | --- | --- | --- |
| SudachiDict small | v20260428 | `https://sudachi.s3.ap-northeast-1.amazonaws.com/sudachidict-raw/20260428/small_lex.zip` | `0c8cc6febab6beac3bb3d7374d74e0123fc16bd93024759808358f63d74b3c09` |
| SudachiDict core | v20260428 | `https://sudachi.s3.ap-northeast-1.amazonaws.com/sudachidict-raw/20260428/core_lex.zip` | `e826394f06b19a699811058f3096629193cf90c8a9b37e2edeaf1e30c76a71d1` |
| JMdict English | generated 2026-09-27 | `https://www.edrdg.org/pub/Nihongo/JMdict_e.gz` | `d9f3c2e58c83f10ca7a6f2b6305e84cd8ac211740ebcee4e73c142d3068aec3f` |

Generated TSV SHA-256: `26b7b4d11ee4efdcd802f1c0284a679aa41cec3e4383f6ef34be453f018b6cbf`.

## Refresh procedure

EDRDG requires a procedure for regular updates. Check both upstream sources each month, download the current SudachiDict small/core source archives and JMdict_e.gz, record their versions and SHA-256 values here, then regenerate:

```sh
python3 scripts/update-tts-presets.py --small /path/to/small_lex.zip --core /path/to/core_lex.zip --jmdict /path/to/JMdict_e.gz
```

Review the changed count and sample readings, run the TTS dictionary and audio tests, and update the bundled licence/documentation copies. Keep the original archives outside this repository. The resulting TSV remains subject to the source licences; this repository's MIT licence does not replace them.
