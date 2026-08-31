const SAFE_PROTOCOL = /^(?:https?:|mailto:)/i;
const INLINE_TOKEN = /`([^`\n]+)`|\[([^\]\n]{1,1024})\]\(([^)\s]{1,2048})\)|\*\*([^*\n]+)\*\*|\*([^*\n]+)\*|~~([^~\n]+)~~/g;

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function renderInline(value: string): string {
  let output = "";
  let cursor = 0;
  for (const match of value.matchAll(INLINE_TOKEN)) {
    const matchIndex = match.index;
    output += escapeHtml(value.slice(cursor, matchIndex));
    if (match[1] !== undefined) {
      output += `<code>${escapeHtml(match[1])}</code>`;
    } else if (match[2] !== undefined && match[3] !== undefined) {
      output += SAFE_PROTOCOL.test(match[3])
        ? `<a href="${escapeHtml(match[3])}" rel="noreferrer noopener" target="_blank">${renderInline(match[2])}</a>`
        : escapeHtml(match[0]);
    } else if (match[4] !== undefined) {
      output += `<strong>${renderInline(match[4])}</strong>`;
    } else if (match[5] !== undefined) {
      output += `<em>${renderInline(match[5])}</em>`;
    } else if (match[6] !== undefined) {
      output += `<del>${renderInline(match[6])}</del>`;
    }
    cursor = matchIndex + match[0].length;
  }
  return output + escapeHtml(value.slice(cursor));
}

function isTableDivider(line: string): boolean {
  const cells = line.trim().replace(/^\||\|$/g, "").split("|");
  return cells.length > 0 && cells.every((cell) => /^\s*:?-{3,}:?\s*$/.test(cell));
}

function tableCells(line: string): string[] {
  return line.trim().replace(/^\||\|$/g, "").split("|").map((cell) => cell.trim());
}

/**
 * A bounded, safe Markdown projection for completed chat messages. Raw HTML is
 * always escaped; only the small tag set emitted here can reach the WebView.
 */
export function renderSafeMarkdown(content: string): string {
  const lines = content.replace(/\r\n/g, "\n").replace(/\r/g, "\n").split("\n");
  const blocks: string[] = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index];
    if (!line.trim()) {
      index += 1;
      continue;
    }
    const fence = line.match(/^\s*```([A-Za-z0-9_+-]{0,40})\s*$/);
    if (fence) {
      const code: string[] = [];
      index += 1;
      while (index < lines.length && !/^\s*```\s*$/.test(lines[index])) {
        code.push(lines[index]);
        index += 1;
      }
      if (index < lines.length) index += 1;
      const language = fence[1] ? ` class="language-${escapeHtml(fence[1])}"` : "";
      blocks.push(`<pre><code${language}>${escapeHtml(code.join("\n"))}</code></pre>`);
      continue;
    }
    if (index + 1 < lines.length && line.includes("|") && isTableDivider(lines[index + 1])) {
      const headers = tableCells(line);
      index += 2;
      const rows: string[][] = [];
      while (index < lines.length && lines[index].includes("|") && lines[index].trim()) {
        rows.push(tableCells(lines[index]));
        index += 1;
      }
      blocks.push(`<table><thead><tr>${headers.map((cell) => `<th>${renderInline(cell)}</th>`).join("")}</tr></thead><tbody>${rows.map((row) => `<tr>${headers.map((_, cellIndex) => `<td>${renderInline(row[cellIndex] ?? "")}</td>`).join("")}</tr>`).join("")}</tbody></table>`);
      continue;
    }
    const heading = line.match(/^(#{1,6})\s+(.+)$/);
    if (heading) {
      const level = heading[1].length;
      blocks.push(`<h${level}>${renderInline(heading[2])}</h${level}>`);
      index += 1;
      continue;
    }
    if (/^\s*[-*+]\s+/.test(line)) {
      const items: string[] = [];
      while (index < lines.length) {
        const item = lines[index].match(/^\s*[-*+]\s+(.+)$/);
        if (!item) break;
        items.push(`<li>${renderInline(item[1])}</li>`);
        index += 1;
      }
      blocks.push(`<ul>${items.join("")}</ul>`);
      continue;
    }
    if (/^\s*\d+[.)]\s+/.test(line)) {
      const items: string[] = [];
      while (index < lines.length) {
        const item = lines[index].match(/^\s*\d+[.)]\s+(.+)$/);
        if (!item) break;
        items.push(`<li>${renderInline(item[1])}</li>`);
        index += 1;
      }
      blocks.push(`<ol>${items.join("")}</ol>`);
      continue;
    }
    if (/^\s*>\s?/.test(line)) {
      const quote: string[] = [];
      while (index < lines.length) {
        const part = lines[index].match(/^\s*>\s?(.*)$/);
        if (!part) break;
        quote.push(part[1]);
        index += 1;
      }
      blocks.push(`<blockquote><p>${renderInline(quote.join("\n")).replace(/\n/g, "<br>")}</p></blockquote>`);
      continue;
    }
    const paragraph = [line];
    index += 1;
    while (index < lines.length && lines[index].trim()
      && !/^\s*```/.test(lines[index])
      && !/^(#{1,6})\s+/.test(lines[index])
      && !/^\s*(?:[-*+]|\d+[.)])\s+/.test(lines[index])
      && !/^\s*>/.test(lines[index])) {
      paragraph.push(lines[index]);
      index += 1;
    }
    blocks.push(`<p>${renderInline(paragraph.join("\n")).replace(/\n/g, "<br>")}</p>`);
  }
  return blocks.join("");
}
