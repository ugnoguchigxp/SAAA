const URL_PATTERN = /(?:https?:\/\/|www\.)[^\s<>()]+/giu;
const EMOJI_PATTERN = /[\p{Extended_Pictographic}\uFE0F\u200D]/gu;

export function toSpeakableText(input: string): string {
  return input
    .replace(/```[\s\S]*?```/g, " コードブロックは省略します。 ")
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(URL_PATTERN, " ")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/^\s{0,3}(?:#{1,6}|>|[-+*]|\d+[.)])\s+/gmu, "")
    .replace(/[*_~]{1,3}/g, "")
    .replace(/<[^>]+>/g, " ")
    .replace(EMOJI_PATTERN, "")
    .replace(/[ \t]+/g, " ")
    .replace(/ *\n+ */g, "。")
    .replace(/。{2,}/g, "。")
    .trim();
}
