# Third-party licenses

SAAA's own source is licensed under [MIT](LICENSE). Dependencies and bundled assets retain their respective licenses; the root MIT license does not replace those terms.

## Bundled voice assets

The [voice runtime notices](src-tauri/resources/voice/THIRD_PARTY_NOTICES.md) identify sherpa-onnx, ONNX Runtime, and the CAMPPlus speaker model, including upstream references and pinned checksums. Preserve these notices when redistributing the corresponding assets.

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
