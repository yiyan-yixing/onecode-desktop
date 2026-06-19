// 忠实移植 cc_status.rs 的 frontmatter_body / fm_field。
// 来源：onecode-desktop/src-tauri/src/cc_status.rs:320-345

// 提取首行 --- 与下一个 --- 之间的正文。无则返回 null（移植 frontmatter_body）。
export function frontmatterBody(content) {
  const lines = String(content).split('\n');
  if (lines.length === 0) return null;
  if (lines[0].trim() !== '---') return null;
  const body = [];
  for (let i = 1; i < lines.length; i++) {
    if (lines[i].trim() === '---') return body.join('\n');
    body.push(lines[i]);
  }
  return null;
}

// 从 fm 正文中取 `key:` 字段值（移植 fm_field）。
// 注意 Rust：line.trim_start().strip_prefix("key:") 再 .trim()。
// 即只要某行（去前导空格后）以 "key:" 开头即命中，取其后整行 trim。
export function fmField(fm, key) {
  const prefix = `${key}:`;
  for (const line of String(fm).split('\n')) {
    const trimmedStart = line.trimStart();
    if (trimmedStart.startsWith(prefix)) {
      return trimmedStart.slice(prefix.length).trim();
    }
  }
  return '';
}
