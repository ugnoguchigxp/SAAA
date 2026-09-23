export type AnswerUrl = { url: string; label: string };

const FENCE = /```[\s\S]*?```/g;
const INLINE_CODE = /`[^`\n]*`/g;
const IMAGE = /!\[[^\]]*]\([^)]*\)/g;
const LINK = /\[([^\]]*)\]\((https?:\/\/[^)\s]+)\)/g;
const PLAIN = /https?:\/\/[^\s<>)\]]+/g;

export function normalizeAnswerUrl(raw: string): string | null {
  const trimmed = raw.replace(/[.,;:!?)]+$/g, "");
  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch {
    return null;
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
  parsed.hash = "";
  return parsed.href;
}

export function extractAnswerUrls(markdown: string): AnswerUrl[] {
  const withoutFences = markdown.replace(FENCE, " ").replace(INLINE_CODE, " ");
  const links: Array<AnswerUrl & { offset: number }> = [];
  const consumed = new Set<string>();
  const body = withoutFences.replace(IMAGE, " ").replace(LINK, (match, label: string, href: string, offset: number) => {
    const url = normalizeAnswerUrl(href);
    if (url && !consumed.has(url)) {
      consumed.add(url);
      const text = label.trim();
      links.push({ url, label: text || new URL(url).host, offset });
    }
    return " ".repeat(match.length);
  });
  for (const match of body.matchAll(PLAIN)) {
    const url = normalizeAnswerUrl(match[0]);
    if (!url || consumed.has(url)) continue;
    consumed.add(url);
    links.push({ url, label: new URL(url).host, offset: match.index });
  }
  return links.sort((a, b) => a.offset - b.offset).map(({ url, label }) => ({ url, label }));
}
