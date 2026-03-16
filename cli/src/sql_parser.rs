// Copyright 2021 Datafuse Labs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use databend_common_ast::parser::token::{TokenKind, Tokenizer};

/// SQL parser utility for splitting SQL text into individual statements
pub struct SqlParser {
    delimiter: char,
    multi_line: bool,
    is_repl: bool,
}

impl SqlParser {
    pub fn new(delimiter: char, multi_line: bool, is_repl: bool) -> Self {
        Self {
            delimiter,
            multi_line,
            is_repl,
        }
    }

    /// Parse SQL text and return a vector of individual SQL statements
    pub fn parse(&self, sql_text: &str) -> Vec<String> {
        let mut queries = Vec::new();
        let mut current_query = String::new();

        for line in sql_text.lines() {
            let line = line.trim();

            if line.is_empty() {
                continue;
            }

            // Handle special commands for REPL mode
            if current_query.is_empty()
                && (line.starts_with('!')
                    || line == "exit"
                    || line == "quit"
                    || line.to_uppercase().starts_with("PUT"))
            {
                queries.push(line.to_owned());
                continue;
            }

            // Handle single line mode
            if !self.multi_line {
                if line.starts_with("--") {
                    continue;
                } else {
                    queries.push(line.to_owned());
                    continue;
                }
            }

            // Append line to current query
            if !current_query.is_empty() {
                current_query.push('\n');
            }
            current_query.push_str(line);

            // Parse the accumulated query to find statement boundaries
            let parsed = self.parse_statements(&current_query);
            for statement in parsed.statements {
                queries.push(statement);
            }
            current_query = parsed.remaining;
        }

        // Add any remaining query
        if !current_query.is_empty() {
            let trimmed = current_query.trim();
            if !trimmed.is_empty() && trimmed != self.delimiter.to_string() {
                queries.push(trimmed.to_string());
            }
        }

        queries
    }

    /// Parse a single line incrementally, maintaining state
    /// Returns complete statements and updates the provided buffer
    pub fn parse_line(
        &self,
        line: &str,
        query_buffer: &mut String,
        err: &mut String,
    ) -> Vec<String> {
        if line.is_empty() {
            return vec![];
        }

        // Handle special commands for REPL mode
        if query_buffer.is_empty()
            && (line.starts_with('!')
                || line == "exit"
                || line == "quit"
                || line.to_uppercase().starts_with("PUT"))
        {
            return vec![line.to_owned()];
        }

        // Handle single line mode
        if !self.multi_line {
            if line.starts_with("--") {
                return vec![];
            } else {
                return vec![line.to_owned()];
            }
        }

        // Append line to query buffer
        if !query_buffer.is_empty() {
            query_buffer.push('\n');
        }
        query_buffer.push_str(line);

        // Parse the accumulated query to find statement boundaries
        let parsed = self.parse_statements(query_buffer);

        *err = parsed.err;
        *query_buffer = parsed.remaining;

        parsed.statements
    }

    /// Parse accumulated query text to extract complete statements
    fn parse_statements(&self, query: &str) -> ParseResult {
        let split = split_statements(query, self.delimiter, self.is_repl);
        ParseResult {
            statements: split.statements,
            remaining: split.remaining,
            err: split.err,
        }
    }
}

struct ParseResult {
    statements: Vec<String>,
    remaining: String,
    err: String,
}

struct SplitResult {
    statements: Vec<String>,
    remaining: String,
    err: String,
}

#[derive(Clone, Copy, PartialEq)]
enum TailState {
    Normal,
    SingleQuote(usize),
    DoubleQuote(usize),
    Backtick(usize),
    DollarQuote(usize),
    BlockComment(usize),
    LineComment,
}

fn unfinished_tail_start(s: &str) -> Option<usize> {
    let mut chars = s.char_indices().peekable();
    let mut state = TailState::Normal;

    while let Some((idx, ch)) = chars.next() {
        match state {
            TailState::Normal => match ch {
                '\'' => state = TailState::SingleQuote(idx),
                '"' => state = TailState::DoubleQuote(idx),
                '`' => state = TailState::Backtick(idx),
                '-' if matches!(chars.peek(), Some((_, '-'))) => {
                    chars.next();
                    state = TailState::LineComment;
                }
                '/' if matches!(chars.peek(), Some((_, '*'))) => {
                    chars.next();
                    state = TailState::BlockComment(idx);
                }
                '$' if matches!(chars.peek(), Some((_, '$'))) => {
                    chars.next();
                    state = TailState::DollarQuote(idx);
                }
                _ => {}
            },
            TailState::SingleQuote(_) => match ch {
                '\\' => {
                    chars.next();
                }
                '\'' if matches!(chars.peek(), Some((_, '\''))) => {
                    chars.next();
                }
                '\'' => state = TailState::Normal,
                _ => {}
            },
            TailState::DoubleQuote(_) => match ch {
                '\\' => {
                    chars.next();
                }
                '"' if matches!(chars.peek(), Some((_, '"'))) => {
                    chars.next();
                }
                '"' => state = TailState::Normal,
                _ => {}
            },
            TailState::Backtick(_) => {
                if ch == '`' {
                    state = TailState::Normal;
                }
            }
            TailState::DollarQuote(_) => {
                if ch == '$' && matches!(chars.peek(), Some((_, '$'))) {
                    chars.next();
                    state = TailState::Normal;
                }
            }
            TailState::BlockComment(_) => {
                if ch == '*' && matches!(chars.peek(), Some((_, '/'))) {
                    chars.next();
                    state = TailState::Normal;
                }
            }
            TailState::LineComment => {
                if matches!(ch, '\n' | '\u{000C}') {
                    state = TailState::Normal;
                }
            }
        }
    }

    match state {
        TailState::SingleQuote(start)
        | TailState::DoubleQuote(start)
        | TailState::Backtick(start)
        | TailState::DollarQuote(start)
        | TailState::BlockComment(start) => Some(start),
        TailState::Normal | TailState::LineComment => None,
    }
}

fn split_statements(s: &str, delimiter: char, is_repl: bool) -> SplitResult {
    let (to_parse, tail) = match unfinished_tail_start(s) {
        Some(pos) => (&s[..pos], &s[pos..]),
        None => (s, ""),
    };

    let delimiter_text = delimiter.to_string();
    let mut statements = Vec::new();
    let mut remaining_query = to_parse.to_string();
    let mut err = String::new();

    'parser: loop {
        let mut tokenizer = Tokenizer::new(&remaining_query).peekable();
        let mut previous_token_backslash = false;
        let mut backslash_start = None;

        while let Some(token) = tokenizer.next() {
            match token {
                Ok(token) => {
                    let token_start = token.span.start as usize;
                    let token_end = token.span.end as usize;
                    let is_delimiter = token.text() == delimiter_text;
                    let is_slash_g = if is_repl
                        && previous_token_backslash
                        && token.kind == TokenKind::Ident
                        && token.text() == "G"
                    {
                        match tokenizer.peek() {
                            Some(Ok(next)) => next.kind == TokenKind::EOI,
                            None => true,
                            Some(Err(_)) => false,
                        }
                    } else {
                        false
                    };

                    if is_delimiter || is_slash_g {
                        let statement_end = if is_slash_g {
                            backslash_start.unwrap_or(token_start)
                        } else {
                            token_start
                        };
                        let sql = remaining_query[..statement_end].trim();
                        if !sql.is_empty() {
                            statements.push(sql.to_string());
                        }
                        remaining_query = remaining_query[token_end..].to_string();
                        continue 'parser;
                    }

                    previous_token_backslash = token.kind == TokenKind::Backslash;
                    backslash_start = previous_token_backslash.then_some(token_start);
                }
                Err(e) => {
                    err = e.to_string();
                    break 'parser;
                }
            }
        }

        break;
    }

    if !tail.is_empty() {
        remaining_query.push_str(tail);
    }

    SplitResult {
        statements,
        remaining: remaining_query.trim().to_string(),
        err,
    }
}

/// Parse SQL text for web API (non-REPL mode)
pub fn parse_sql_for_web(sql_text: &str) -> Vec<String> {
    let parser = SqlParser::new(';', true, false);
    parser.parse(sql_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(s: &str) -> SplitResult {
        split_statements(s, ';', false)
    }

    fn split_repl(s: &str) -> SplitResult {
        split_statements(s, ';', true)
    }

    #[test]
    fn basic_semicolon_split() {
        let r = split("SELECT 1; SELECT 2;");
        assert_eq!(r.statements, vec!["SELECT 1", "SELECT 2"]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn semicolon_inside_single_quotes() {
        let r = split("SELECT 'a;b';");
        assert_eq!(r.statements, vec!["SELECT 'a;b'"]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn single_quote_backslash_escape() {
        let r = split(r"SELECT 'it\'s a;test';");
        assert_eq!(r.statements, vec![r"SELECT 'it\'s a;test'"]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn single_quote_backslash_escape_with_unicode() {
        let r = split("SELECT 'a\\é;b';");
        assert_eq!(r.statements, vec!["SELECT 'a\\é;b'"]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn single_quote_doubled_escape() {
        let r = split("SELECT 'it''s a;test';");
        assert_eq!(r.statements, vec!["SELECT 'it''s a;test'"]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn semicolon_inside_double_quotes() {
        let r = split(r#"SELECT "a;b";"#);
        assert_eq!(r.statements, vec![r#"SELECT "a;b""#]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn double_quote_backslash_escape_with_unicode() {
        let r = split("SELECT \"a\\é;b\";");
        assert_eq!(r.statements, vec!["SELECT \"a\\é;b\""]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn semicolon_inside_backticks() {
        let r = split("SELECT `a;b`;");
        assert_eq!(r.statements, vec!["SELECT `a;b`"]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn semicolon_inside_dollar_quote() {
        let r = split("SELECT $$a;b$$;");
        assert_eq!(r.statements, vec!["SELECT $$a;b$$"]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn semicolon_inside_block_comment() {
        let r = split("SELECT /* ; */ 1;");
        assert_eq!(r.statements, vec!["SELECT /* ; */ 1"]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn semicolon_inside_line_comment() {
        let r = split("SELECT 1 -- ;\n;");
        assert_eq!(r.statements, vec!["SELECT 1 -- ;"]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn unclosed_block_comment_repl() {
        let r = split_repl("SELECT 1; /*");
        assert_eq!(r.statements, vec!["SELECT 1"]);
        assert_eq!(r.remaining, "/*");
    }

    #[test]
    fn unclosed_single_quote_repl() {
        let r = split_repl("SELECT '");
        assert_eq!(r.statements, Vec::<String>::new());
        assert_eq!(r.remaining, "SELECT '");
    }

    #[test]
    fn unclosed_dollar_quote_repl() {
        let r = split_repl("SELECT $$");
        assert_eq!(r.statements, Vec::<String>::new());
        assert_eq!(r.remaining, "SELECT $$");
    }

    #[test]
    fn backslash_g_repl() {
        let r = split_repl(r"SELECT 1\G");
        assert_eq!(r.statements, vec!["SELECT 1"]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn backslash_g_repl_requires_end_of_input() {
        let r = split_repl(r"SELECT 1\Gfoo");
        assert_eq!(r.statements, Vec::<String>::new());
        assert_eq!(r.remaining, r"SELECT 1\Gfoo");
    }

    #[test]
    fn backslash_g_non_repl_ignored() {
        let r = split(r"SELECT 1\G;");
        assert_eq!(r.statements, vec![r"SELECT 1\G"]);
    }

    #[test]
    fn empty_input() {
        let r = split("");
        assert_eq!(r.statements, Vec::<String>::new());
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn only_semicolons() {
        let r = split(";;;");
        assert_eq!(r.statements, Vec::<String>::new());
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn only_whitespace() {
        let r = split("   \n\t  ");
        assert_eq!(r.statements, Vec::<String>::new());
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn mixed_quotes_and_comments() {
        let r = split("SELECT 'a' /* comment */ , \"b;c\" , `d;e`; SELECT $$f;g$$;");
        assert_eq!(
            r.statements,
            vec![
                "SELECT 'a' /* comment */ , \"b;c\" , `d;e`",
                "SELECT $$f;g$$",
            ]
        );
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn no_trailing_semicolon() {
        let r = split("SELECT 1");
        assert_eq!(r.statements, Vec::<String>::new());
        assert_eq!(r.remaining, "SELECT 1");
    }

    #[test]
    fn unclosed_block_comment_with_preceding_stmt() {
        let r = split("SELECT 1; SELECT 2 /*");
        assert_eq!(r.statements, vec!["SELECT 1"]);
        assert_eq!(r.remaining, "SELECT 2 /*");
    }

    #[test]
    fn delimiter_dollar_does_not_break_dollar_quote() {
        let r = split_statements("SELECT $$a$b$$$", '$', false);
        assert_eq!(r.statements, vec!["SELECT $$a$b$$"]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn delimiter_dollar_does_not_break_placeholders_or_variables() {
        let r = split_statements("SELECT $1 $", '$', false);
        assert_eq!(r.statements, vec!["SELECT $1"]);
        assert_eq!(r.remaining, "");

        let r = split_statements("SELECT $foo $", '$', false);
        assert_eq!(r.statements, vec!["SELECT $foo"]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn delimiter_at_does_not_break_stage_literal() {
        let r = split_statements("COPY INTO t FROM @~/stage/file @", '@', false);
        assert_eq!(r.statements, vec!["COPY INTO t FROM @~/stage/file"]);
        assert_eq!(r.remaining, "");
    }

    #[test]
    fn delimiter_slash_does_not_break_block_comment() {
        let r = split_statements("SELECT /* c */ 1/", '/', false);
        assert_eq!(r.statements, vec!["SELECT /* c */ 1"]);
        assert_eq!(r.remaining, "");
    }
}
