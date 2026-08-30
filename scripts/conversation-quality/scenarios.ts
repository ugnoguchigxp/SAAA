export type ToolMode = "none" | "success" | "failure";

export type QualityScenario = {
  id: string;
  category: "ja" | "en" | "ambiguous-asr" | "long" | "current" | "tool-failure" | "continuation";
  language: "ja" | "en";
  input: string;
  expected: string;
  toolMode: ToolMode;
  toolResult?: string;
};

function cases(
  category: QualityScenario["category"],
  language: QualityScenario["language"],
  inputs: Array<[string, string]>,
  toolMode: ToolMode = "none",
  toolResult?: string,
): QualityScenario[] {
  return inputs.map(([input, expected], index) => ({
    id: `${category}-${String(index + 1).padStart(2, "0")}`,
    category,
    language,
    input,
    expected,
    toolMode,
    toolResult,
  }));
}

export const QUALITY_SCENARIOS: QualityScenario[] = [
  ...cases("ja", "ja", [
    ["7×8はいくつですか。", "56と端的に答える"],
    ["会議を15分短くする案を2つください。", "実行可能な案を2つだけ示す"],
    ["この文を丁寧にしてください。明日までに返して。", "意味を変えず丁寧な日本語へ直す"],
    ["水を1.5リットル、6人で等分すると一人何mlですか。", "250mlと計算する"],
    ["雨の日の持ち物を3つだけ教えて。", "日本語で3項目だけ答える"],
    ["『了解しました』をもう少し柔らかく言い換えて。", "自然で柔らかい短文を返す"],
    ["午前9時の90分後は何時ですか。", "午前10時30分と答える"],
    ["次の文の要点を一文にしてください。品質を上げるには、失敗を隠さず計測し、原因ごとに直す必要があります。", "計測と原因別改善を一文で要約する"],
    ["A案は3日、B案は5日です。早い方はどちらですか。", "A案と答える"],
    ["今日は集中できません。最初の一歩を一つだけ提案して。", "負担の小さい具体策を一つ示す"],
  ]),
  ...cases("en", "en", [
    ["What is 12 divided by 3?", "Answer 4 concisely in English"],
    ["Give me two ways to make a meeting shorter.", "Give exactly two practical suggestions"],
    ["Rewrite politely: Send this by Friday.", "Return a polite English rewrite"],
    ["How many minutes are in 2.5 hours?", "Answer 150 minutes"],
    ["Name three items to pack for rain, and nothing else.", "List exactly three relevant items"],
    ["Summarize in one sentence: Good systems expose failures and make recovery explicit.", "Preserve the main point in one sentence"],
    ["Which is faster: a 3-day option or a 5-day option?", "Answer the 3-day option"],
    ["Make this friendlier: I cannot attend.", "Return a natural friendly rewrite"],
    ["What time is 45 minutes after 10:30?", "Answer 11:15"],
    ["I feel stuck. Suggest one tiny next step.", "Give one small actionable step"],
  ]),
  ...cases("ambiguous-asr", "ja", [
    ["明日の会議、資料はカイギまででいい？", "締切の意味が不明なので一つだけ短く確認する"],
    ["3時に佐藤さんへ電話して。", "十分明確なので確認を増やさず内容を復唱する"],
    ["予算を15か50にして。", "金額や単位が重大に曖昧なので短く確認する"],
    ["東京駅までの行き方を教えて。", "出発地が必要なので一つだけ確認する"],
    ["牛乳を2本買うメモにして。", "明確なので短いメモを返す"],
    ["来週の木曜か金曜に30分とって。", "選択が必要なので曜日を一つだけ確認する"],
    ["アカウントを削除して。", "対象と不可逆性が重大なので実行を断定せず確認する"],
    ["音量を少し下げて。", "曖昧さを誇張せず簡潔に応じる"],
    ["請求書を青木さんに送る文面を作って。", "外部送信はせず文面だけを作る"],
    ["次の電車、何分？", "駅や方面が不足しているため一つだけ確認する"],
  ]),
  ...cases("long", "ja", [
    ["在宅勤務の利点と注意点を、結論を先にして4項目以内で説明してください。", "結論先行で4項目以内、反復しない"],
    ["バックアップ運用を初めて担当する人向けに、毎週の確認手順を説明してください。", "順序が明確で実行可能、過剰な前置きなし"],
    ["小規模チームが障害対応を改善する方法を300字以内で教えて。", "300字以内で具体的な改善策を示す"],
    ["パスワード管理ツールを導入する際の判断基準を説明して。", "安全性と運用性を偏りなく簡潔に示す"],
    ["新機能の段階リリースが必要な理由を、非エンジニアにも分かるように説明して。", "専門用語を抑え段階リリースの価値を説明する"],
    ["音声アプリで遅延と精度のどちらを優先すべきか、判断軸を示して。", "一律に断定せず用途別の判断軸を示す"],
    ["失敗した処理の再試行設計で注意することを説明して。", "重複防止、範囲、状態を含める"],
    ["ユーザー設定画面を分かりやすくする原則を、例を交えて説明して。", "実際に変更可能な値と状態表示を区別する"],
  ]),
  ...cases("current", "ja", [
    ["架空市の今日の天気を教えて。", "検索結果だけを根拠に天気を答える"],
    ["SAAAリーグの最新順位は？", "検索結果だけを根拠に順位を答える"],
    ["ABC社の現在のCEOは誰ですか。", "検索結果だけを根拠に人物を答える"],
    ["今日のUSD/JPYの参考値を教えて。", "時点を添え検索結果だけを答える"],
    ["今週末の架空ホールの公演予定は？", "検索結果にある予定だけを答える"],
    ["最新版のSAAA OSは何版？", "検索結果にある版だけを答える"],
    ["架空線の現在の運行状況を教えて。", "検索結果にある運行状況だけを答える"],
    ["今月の架空市のごみ収集日を教えて。", "検索結果にある日付だけを答える"],
  ], "success", "Deterministic test search result: requested item is Sample A, updated 2026-08-30 09:00 JST."),
  ...cases("tool-failure", "ja", [
    ["架空市の今の気温は？", "検索失敗を明示し気温を推測しない"],
    ["現在の株価を教えて。", "取得失敗を明示し価格を捏造しない"],
    ["今日のニュースの見出しは？", "取得失敗を明示し見出しを捏造しない"],
    ["次の電車は何時？", "取得失敗と必要な条件を簡潔に伝える"],
    ["今営業中の店を教えて。", "取得失敗を明示し店舗を捏造しない"],
    ["最新バージョンを教えて。", "取得失敗を明示し版を断定しない"],
    ["明日の試合開始時刻は？", "取得失敗を明示し時刻を捏造しない"],
  ], "failure", "Tool error: deterministic network failure."),
  ...cases("continuation", "ja", [
    ["回答が途中で切れました。重複せず、バックアップ確認の2番目の手順から続けて。", "冒頭を繰り返さず指定位置から続ける"],
    ["さっきの箇条書きの4番目だけ続けて。", "不明な履歴を捏造せず必要なら簡潔に確認する"],
    ["『まず設定を保存します。次に』の続きだけ書いて。", "与えられた文を反復せず自然に続ける"],
    ["説明の結論だけが切れました。結論を一文で補って。", "一文の結論だけを返す"],
    ["同じ内容を繰り返さず、残りの注意点を2つ。", "重複を避け2点だけ返す"],
    ["英語の回答の続きを英語で一文だけ。", "必要情報がなければ短く確認し言語を守る"],
    ["コードは再掲せず、次に実行するコマンドだけ教えて。", "コードを再掲せず必要なら文脈を確認する"],
  ]),
];
