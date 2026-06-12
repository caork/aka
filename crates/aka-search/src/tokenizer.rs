//! 代码感知 tokenizer — 标识符按 camelCase / snake_case / kebab-case / 数字边界
//! 拆分子词，同时保留原始完整 token；全部 lowercase。
//!
//! 例：`runPipelineFromRepo` → `[runpipelinefromrepo, run, pipeline, from, repo]`；
//! `kernel_add` → `[kernel_add, kernel, add]`；`HTTPServer` → `[httpserver, http, server]`。

use tantivy::tokenizer::{Token, TokenStream, Tokenizer};

/// 注册到 tantivy TokenizerManager 时使用的名字。
pub const CODE_TOKENIZER_NAME: &str = "code";

/// 代码感知 tokenizer。
///
/// 外层先把文本切成最大连续的"标识符字符"（字母数字 + `_` + `-`）片段，
/// 每个片段先输出完整 token（小写），再输出按以下边界拆出的子词（小写）：
///
/// - `_` / `-` 分隔符（snake_case / kebab-case）
/// - 小写→大写转折（camelCase）
/// - 连续大写后跟小写（`HTTPServer` → `HTTP` + `Server`）
/// - 字母↔数字边界（`utf8` → `utf` + `8`）
///
/// 只有一个子词（即子词等于完整 token）时不重复输出。
#[derive(Clone, Default)]
pub struct CodeTokenizer;

/// [`CodeTokenizer`] 产出的 token 流（构造时一次性切完）。
pub struct CodeTokenStream {
    tokens: Vec<Token>,
    /// 已输出 token 数；`token()` 返回 `tokens[index - 1]`。
    index: usize,
}

impl Tokenizer for CodeTokenizer {
    type TokenStream<'a> = CodeTokenStream;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> CodeTokenStream {
        CodeTokenStream {
            tokens: tokenize(text),
            index: 0,
        }
    }
}

impl TokenStream for CodeTokenStream {
    fn advance(&mut self) -> bool {
        if self.index < self.tokens.len() {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn token(&self) -> &Token {
        &self.tokens[self.index - 1]
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.tokens[self.index - 1]
    }
}

/// 标识符字符：字母数字 + `_` + `-`（`-` 为支持 kebab-case 整词保留）。
fn is_token_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// 把整段文本切成 token 序列（完整 token + 子词，全小写）。
fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut iter = text.char_indices().peekable();

    while let Some(&(start, c)) = iter.peek() {
        if !is_token_char(c) {
            iter.next();
            continue;
        }
        // 吃掉最大连续标识符字符片段 [start, end)。
        let mut end = text.len();
        while let Some(&(off, ch)) = iter.peek() {
            if is_token_char(ch) {
                iter.next();
            } else {
                end = off;
                break;
            }
        }
        emit_word(text, start, end, &mut tokens);
    }
    tokens
}

/// 对一个原始片段输出完整 token + 子词。
fn emit_word(text: &str, start: usize, end: usize, tokens: &mut Vec<Token>) {
    let raw = &text[start..end];
    // 去掉首尾的分隔符（`_foo_` → `foo`；纯分隔符片段丢弃）。
    let trimmed = raw.trim_matches(|c| c == '_' || c == '-');
    if trimmed.is_empty() {
        return;
    }
    let t_start = start + (trimmed.as_ptr() as usize - raw.as_ptr() as usize);
    let t_end = t_start + trimmed.len();

    push_token(tokens, trimmed.to_lowercase(), t_start, t_end);

    let subs = subword_ranges(trimmed);
    if subs.len() > 1 {
        for (s, e) in subs {
            push_token(
                tokens,
                trimmed[s..e].to_lowercase(),
                t_start + s,
                t_start + e,
            );
        }
    }
}

fn push_token(tokens: &mut Vec<Token>, text: String, offset_from: usize, offset_to: usize) {
    let position = tokens.len();
    tokens.push(Token {
        offset_from,
        offset_to,
        position,
        text,
        position_length: 1,
    });
}

#[derive(PartialEq, Clone, Copy)]
enum CharKind {
    Lower,
    Upper,
    Digit,
    Sep,
}

fn kind(c: char) -> CharKind {
    if c == '_' || c == '-' {
        CharKind::Sep
    } else if c.is_numeric() {
        CharKind::Digit
    } else if c.is_uppercase() {
        CharKind::Upper
    } else {
        // 小写字母与无大小写文字（CJK 等）统一按 Lower 处理。
        CharKind::Lower
    }
}

/// 返回 `word` 内各子词的字节区间（不含分隔符）。
fn subword_ranges(word: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    let mut ranges = Vec::new();
    let mut start: Option<usize> = None;

    for i in 0..chars.len() {
        let (off, c) = chars[i];
        let k = kind(c);
        if k == CharKind::Sep {
            if let Some(s) = start.take() {
                ranges.push((s, off));
            }
            continue;
        }
        match start {
            None => start = Some(off),
            Some(s) => {
                let pk = kind(chars[i - 1].1);
                let boundary = match (pk, k) {
                    // camelCase：小写后跟大写。
                    (CharKind::Lower, CharKind::Upper) => true,
                    // 连续大写后跟小写：在大写串最后一个字母前断开（HTTPServer → HTTP|Server）。
                    (CharKind::Upper, CharKind::Upper) => {
                        matches!(chars.get(i + 1), Some(&(_, nc)) if kind(nc) == CharKind::Lower)
                    }
                    // 字母↔数字边界。
                    (CharKind::Digit, CharKind::Lower | CharKind::Upper)
                    | (CharKind::Lower | CharKind::Upper, CharKind::Digit) => true,
                    _ => false,
                };
                if boundary {
                    ranges.push((s, off));
                    start = Some(off);
                }
            }
        }
    }
    if let Some(s) = start {
        ranges.push((s, word.len()));
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use tantivy::tokenizer::TextAnalyzer;

    fn toks(text: &str) -> Vec<String> {
        let mut analyzer = TextAnalyzer::from(CodeTokenizer);
        let mut stream = analyzer.token_stream(text);
        let mut out = Vec::new();
        while stream.advance() {
            out.push(stream.token().text.clone());
        }
        out
    }

    #[test]
    fn camel_case_splits_and_keeps_full_token() {
        assert_eq!(
            toks("runPipelineFromRepo"),
            vec!["runpipelinefromrepo", "run", "pipeline", "from", "repo"]
        );
    }

    #[test]
    fn snake_case_splits_and_keeps_full_token() {
        assert_eq!(toks("kernel_add"), vec!["kernel_add", "kernel", "add"]);
    }

    #[test]
    fn consecutive_uppercase_run() {
        assert_eq!(toks("HTTPServer"), vec!["httpserver", "http", "server"]);
    }

    #[test]
    fn digit_boundaries() {
        assert_eq!(toks("utf8Parser"), vec!["utf8parser", "utf", "8", "parser"]);
    }

    #[test]
    fn kebab_case() {
        assert_eq!(
            toks("my-component"),
            vec!["my-component", "my", "component"]
        );
    }

    #[test]
    fn plain_word_not_duplicated() {
        assert_eq!(toks("pipeline"), vec!["pipeline"]);
    }

    #[test]
    fn mixed_source_text() {
        assert_eq!(
            toks("fn kernel_add(a, b)"),
            vec!["fn", "kernel_add", "kernel", "add", "a", "b"]
        );
    }

    #[test]
    fn separator_only_and_trimming() {
        assert_eq!(toks("--- __ _foo_"), vec!["foo"]);
    }

    #[test]
    fn offsets_point_into_original_text() {
        let mut analyzer = TextAnalyzer::from(CodeTokenizer);
        let text = "call runPipelineFromRepo now";
        let mut stream = analyzer.token_stream(text);
        while stream.advance() {
            let t = stream.token();
            let slice = &text[t.offset_from..t.offset_to];
            assert_eq!(slice.to_lowercase(), t.text);
        }
    }
}
