/**
 * Splits pasted text into task prompts. If the text contains a blank line,
 * prompts are separated by blank lines (so a prompt may span several lines);
 * otherwise each non-empty line is its own prompt.
 */
export function parseTaskPrompts(text: string): string[] {
  const normalized = text.replace(/\r\n?/g, '\n').trim();
  if (!normalized) return [];

  const chunks = /\n[ \t]*\n/.test(normalized)
    ? normalized.split(/\n[ \t]*\n+/)
    : normalized.split('\n');

  return chunks.map((chunk) => chunk.trim()).filter(Boolean);
}
