/* 轻量级语法高亮：纯正则 token，不引入任何高亮库。
   实现与 DetailPanel 内置版本一致（CodeView 自用的抽取副本——
   DetailPanel 由另一条线维护，刻意不互相依赖）。 */

import type { CSSProperties, ReactNode } from "react";

const KEYWORDS = new Set(
  (
    "fn let mut pub use mod impl struct enum trait match if else for while loop return " +
    "async await const static type where dyn ref move crate super function class interface " +
    "extends implements import export from new this var void typeof instanceof in of try " +
    "catch finally throw switch case break continue default do yield def lambda pass raise " +
    "with as global elif not and or is None True False null undefined true false package " +
    "func go chan defer select range int string bool float double char long public private " +
    "protected abstract final override readonly namespace virtual delete union self Self"
  ).split(" "),
);

type TokenClass = "comment" | "string" | "number" | "keyword" | "plain";

const TOKEN_COLORS: Record<Exclude<TokenClass, "plain">, CSSProperties> = {
  comment: { color: "var(--ink-3)", fontStyle: "italic" },
  string: { color: "var(--syntax-string)" },
  number: { color: "var(--syntax-number)" },
  keyword: { color: "var(--syntax-keyword)" },
};

const TOKEN_RE =
  /\/\/.*|\/\*.*?(?:\*\/|$)|"(?:\\.|[^"\\])*"?|'(?:\\.|[^'\\])*'?|`(?:\\.|[^`\\])*`?|\b\d[\d_]*(?:\.\d+)?\b|\b[A-Za-z_$][\w$]*\b/g;

/** 以 # 开头注释的语言（按扩展名粗判，误判代价低） */
export function hashComments(file: string): boolean {
  return /\.(py|rb|sh|bash|zsh|pl|yml|yaml|toml|cfg|ini|mk)$|Makefile$|Dockerfile$/i.test(
    file,
  );
}

function classify(tok: string): TokenClass {
  const c0 = tok[0];
  if (c0 === "/" && (tok[1] === "/" || tok[1] === "*")) return "comment";
  if (c0 === '"' || c0 === "'" || c0 === "`") return "string";
  if (c0 >= "0" && c0 <= "9") return "number";
  if (KEYWORDS.has(tok)) return "keyword";
  return "plain";
}

/** 把一段源码文本渲染成着色 token 序列（hash = # 注释语言）。 */
export function renderTokens(text: string, hash: boolean): ReactNode {
  /* # 注释：整行剩余部分视为注释（仅 hash 语言） */
  let head = text;
  let hashTail: string | null = null;
  if (hash) {
    const idx = text.indexOf("#");
    if (idx >= 0) {
      head = text.slice(0, idx);
      hashTail = text.slice(idx);
    }
  }

  const out: ReactNode[] = [];
  let last = 0;
  let key = 0;
  TOKEN_RE.lastIndex = 0;
  for (let m = TOKEN_RE.exec(head); m; m = TOKEN_RE.exec(head)) {
    if (m.index > last) out.push(head.slice(last, m.index));
    const cls = classify(m[0]);
    if (cls === "plain") {
      out.push(m[0]);
    } else {
      out.push(
        <span key={key++} style={TOKEN_COLORS[cls]}>
          {m[0]}
        </span>,
      );
    }
    last = m.index + m[0].length;
  }
  if (last < head.length) out.push(head.slice(last));
  if (hashTail !== null) {
    out.push(
      <span key={key++} style={TOKEN_COLORS.comment}>
        {hashTail}
      </span>,
    );
  }
  return out;
}
