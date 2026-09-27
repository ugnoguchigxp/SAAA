# Third-party licenses

SAAA's own source is licensed under [MIT](LICENSE). Dependencies and bundled assets retain their respective licenses; the root MIT license does not replace those terms.

## Bundled voice assets

The [voice runtime notices](src-tauri/resources/voice/THIRD_PARTY_NOTICES.md) identify sherpa-onnx, ONNX Runtime, and the CAMPPlus speaker model, including upstream references and pinned checksums. Preserve these notices when redistributing the corresponding assets.

## Japanese TTS preset readings

The bundled TTS preset list is derived from [SudachiDict](https://github.com/WorksApplications/SudachiDict) by Works Applications Co., Ltd. and [JMdict](https://www.edrdg.org/wiki/JMdict-EDICT_Dictionary_Project.html) by the Electronic Dictionary Research and Development Group. It contains only written forms with one matching reading in both sources; user-saved readings take precedence. SudachiDict is distributed under Apache License 2.0 with additional notices for incorporated data. JMdict data and data derived from it are subject to CC BY-SA 4.0 and the EDRDG dictionary licence conditions. The [source versions and update procedure](src-tauri/data/tts-presets.SOURCES.md) identify the exact inputs. The bundled [licence and documentation copies](src-tauri/resources/tts-dictionary/) accompany desktop distributions.

## Software dependencies

Dependency versions are recorded in [bun.lock](bun.lock), [src-tauri/Cargo.lock](src-tauri/Cargo.lock), and the lockfiles for standalone Rust packages. Package manifests identify direct dependencies:

- [Frontend and development tools](package.json)
- [Desktop runtime](src-tauri/Cargo.toml)
- [LARM session client](crates/larm-session/Cargo.toml)
- [Reasoning contract](crates/reasoning-contract/Cargo.toml)
- [Reasoning MCP service](services/reasoning-mcp/Cargo.toml)

This page is an index, not a complete generated license inventory or SBOM. For a distribution, collect the license texts and notices from the exact dependency and asset versions included in that build. External model services and user-supplied models are configured separately and are not relicensed by SAAA.

## 日本語

SAAA本体は[MIT](LICENSE)です。依存ソフトウェアや同梱モデルにはそれぞれのライセンスが適用されます。配布するbuildに含まれる実際のバージョンに対応する本文とNOTICEを確認してください。このページは参照先の一覧であり、網羅的なライセンス台帳やSBOMではありません。
