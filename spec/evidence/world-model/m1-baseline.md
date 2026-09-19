# M1 ベースライン

- HEAD: `b3c4606`
- `git status --short`: spec/docs 2件 staged、src-tauri 複数 modified、generated_capabilities 2件 untracked（World作業開始前の既存差分。World作業では所有しない）
- DB版: 20 (`src-tauri/src/persistence/schema.rs`)
- 上限: patch 16KiB / ops 32 / payload 2000B（計画§3どおり）
- core test: 0 tests ok（WM-01で world_contract を追加）
- 失敗ログ: なし（K2/K3/K4は対象カードで初回実行し記録する）
