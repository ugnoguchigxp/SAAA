const URL_PATTERN = /(?:https?:\/\/|www\.)[^\s<>()]+/giu;
const EMOJI_PATTERN = /[\p{Extended_Pictographic}\uFE0F\u200D]/gu;

export function toSpeakableText(input: string): string {
  const japanese = /[\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Han}]/u.test(input);
  const codeReplacement = japanese ? " コードブロックは省略します。 " : " Code block omitted. ";
  const paragraphPause = japanese ? "。" : " ";
  return input
    .replace(/```[\s\S]*?```/g, codeReplacement)
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(URL_PATTERN, " ")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/^\s{0,3}(?:#{1,6}|>|[-+*]|\d+[.)])\s+/gmu, "")
    .replace(/[*_~]{1,3}/g, "")
    .replace(/<[^>]+>/g, " ")
    .replace(EMOJI_PATTERN, "")
    .replace(/[ \t]+/g, " ")
    .replace(/ *\n+ */g, paragraphPause)
    .replace(/。{2,}/g, "。")
    .trim();
}
