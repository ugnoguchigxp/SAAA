const messages: Record<string, string> = {
  coding_configuration_invalid:
    "実装設定を確認してください。専用端末を使う場合は、端末と実装エージェントの両方を選びます。",
  selected_terminal_missing:
    "選択した端末が見つかりません。端末を導入するか、設定で別の端末を選んでください。",
  terminal_cli_missing: "CLIが見つかりません。導入済みの実行ファイルの絶対パスを設定してください。",
  terminal_cli_unavailable: "CLIを起動できません。実行ファイルのパスと導入状態を確認してください。",
  terminal_executable_must_be_absolute: "CLIの実行ファイルは絶対パスで指定してください。",
  terminal_cli_not_executable:
    "指定したCLIを実行できません。実行権限と導入状態を確認してください。",
  terminal_launch_failed: "端末を開けませんでした。選択した端末の導入状態を確認してください。",
  terminal_probe_failed:
    "CLIの起動確認に失敗しました。実行ファイルのパスと導入状態を確認してください。",
  terminal_probe_timeout:
    "CLIの起動確認が時間切れになりました。端末で起動できるか確認してください。",
  stale_revision: "作業の状態が変わりました。最新の表示を確認して、もう一度操作してください。",
  stale_question: "この質問の状態が変わりました。最新の質問を確認してください。",
  question_process_not_stopped:
    "CLIの停止を確認しています。回答できる状態になるまでお待ちください。",
  permission_requires_user_screen: "操作の許可は「作業」画面から回答してください。",
  workspace_not_git: "Gitで管理されているフォルダーを選択してください。",
  workspace_missing: "フォルダーが見つかりません。絶対パスを確認してください。",
};
export function terminalErrorMessage(error: unknown): string {
  const message = String(error);
  return Object.entries(messages).find(([code]) => message.includes(code))?.[1] ?? message;
}
