# Delegated-work acceptance phrases

These fixed, synthetic phrases are used to review proposal routing. They do
not contain personal content or filesystem paths.

## Explicit delegated-work requests (20)

1. 失敗しているテストを調べて、終わったら知らせて — `work_propose`
2. CIで落ちた原因を確認して報告して — `work_propose`
3. この変更のテスト結果を見ておいて — `work_propose`
4. テストを走らせて、結果だけ教えて — `work_propose`
5. エラーの再現条件を調査して — `work_propose`
6. 失敗ログを読んで原因をまとめて — `work_propose`
7. lintの失敗を確認して後で知らせて — `work_propose`
8. 依存更新後のテスト状況を見て — `work_propose`
9. このPRの検証結果を調べて — `work_propose`
10. 型チェックが通るか確認して — `work_propose`
11. 壊れている箇所を調査して報告して — `work_propose`
12. テスト失敗の一覧を取得して — `work_propose`
13. 今夜、テストを確認して結果を残して — `work_propose`
14. 調査だけお願い、修正はしないで — `work_propose`
15. read-onlyで失敗原因を見て — `work_propose`
16. 登録済みのテストrecipeを実行して — `work_propose`
17. 終わったら会議後に知らせて — `work_propose`
18. Aの検証を任せて、Bの作業は止めないで — `work_propose` または対象確認
19. このGoalだけ取り消して — `work_withdraw`（Goal を特定できる場合だけ）
20. 実行中の調査の状態を見せて — `work_status`

## Non-adoption / confirmation cases (10)

1. 「テストを確認して」と書いてある文を引用しただけです
2. テストを確認してはいけない
3. それをお願い
4. 何か良さそうなことをやって
5. 全部直しておいて
6. 本番にデプロイして
7. このURLに送信して
8. 修正も含めて自由に進めて
9. 予算は気にしないで何度でも試して
10. 前の依頼を忘れて

`work_propose` は、選択済み workspace と現在のユーザー source があり、read/test
の対象・予算・完了条件を host が検証できる場合だけ受理する。18 は A/B の対象を
解決できなければ確認へ進める。19 は UI 選択などで Goal を特定できる場合だけ撤回し、
20 は既存の権限を変えない照会として扱う。引用、否定、曖昧参照、外部操作、無制限予算、
write 要求は新しい authority を作らない。
