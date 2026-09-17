//! Syntax colour for SQL and for the expressions the graph renders.
//!
//! One lexer, two vocabularies: SQL keywords for the editor, executed SQL,
//! and fenced code in the conversation; the graph's own words (`when`,
//! `otherwise`, `AND`, `is null`, `ascending`) for stage summaries and
//! facts. Colour is a reading aid, never the only carrier: the text stays
//! complete and copyable, and every kind also differs in weight or style.

use dioxus::prelude::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TokenKind {
    Keyword,
    Function,
    Field,
    Literal,
    Operator,
    Punct,
    Comment,
    Plain,
}

impl TokenKind {
    pub(super) const fn class_name(self) -> &'static str {
        match self {
            Self::Keyword => "tok-keyword",
            Self::Function => "tok-function",
            Self::Field => "tok-field",
            Self::Literal => "tok-literal",
            Self::Operator => "tok-operator",
            Self::Punct => "tok-punct",
            Self::Comment => "tok-comment",
            Self::Plain => "",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Token {
    pub(super) kind: TokenKind,
    pub(super) text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Vocabulary {
    Sql,
    Expression,
}

const SQL_KEYWORDS: [&str; 75] = [
    "SELECT",
    "FROM",
    "WHERE",
    "WITH",
    "AS",
    "JOIN",
    "ON",
    "GROUP",
    "BY",
    "ORDER",
    "PARTITION",
    "OVER",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "AND",
    "OR",
    "NOT",
    "IS",
    "NULL",
    "IN",
    "LIMIT",
    "OFFSET",
    "DISTINCT",
    "CAST",
    "HAVING",
    "UNION",
    "ALL",
    "LEFT",
    "RIGHT",
    "INNER",
    "OUTER",
    "FULL",
    "CROSS",
    "ASC",
    "DESC",
    "NULLS",
    "FIRST",
    "LAST",
    "BETWEEN",
    "LIKE",
    "ILIKE",
    "EXISTS",
    "TRUE",
    "FALSE",
    "INTERVAL",
    "ROWS",
    "RANGE",
    "PRECEDING",
    "FOLLOWING",
    "UNBOUNDED",
    "CURRENT",
    "ROW",
    "USING",
    "LATERAL",
    "VALUES",
    "EXCEPT",
    "INTERSECT",
    "BIGINT",
    "INT",
    "INTEGER",
    "SMALLINT",
    "DOUBLE",
    "FLOAT",
    "REAL",
    "DECIMAL",
    "NUMERIC",
    "VARCHAR",
    "TEXT",
    "STRING",
    "BOOLEAN",
    "TIMESTAMP",
    "DATE",
];

const EXPRESSION_KEYWORDS: [&str; 34] = [
    "when",
    "otherwise",
    "and",
    "or",
    "not",
    "is",
    "null",
    "as",
    "on",
    "true",
    "false",
    "ascending",
    "descending",
    "first",
    "by",
    "ordered",
    "restart",
    "for",
    "each",
    "decimal",
    "i64",
    "i32",
    "u64",
    "u32",
    "f64",
    "f32",
    "utf8",
    "bool",
    "boolean",
    "timestamp",
    "date",
    "bigint",
    "int",
    "text",
];

/// A lexed string. Whitespace and unknown characters stay as plain tokens so
/// joining every token's text reproduces the input exactly.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Tokens(Vec<Token>);

impl Tokens {
    pub(super) fn sql(text: &str) -> Self {
        Self::lex(text, Vocabulary::Sql)
    }

    pub(super) fn expression(text: &str) -> Self {
        Self::lex(text, Vocabulary::Expression)
    }

    #[cfg(test)]
    pub(super) fn tokens(&self) -> &[Token] {
        &self.0
    }

    /// Escaped HTML with one span per coloured token, for markdown output that
    /// is sanitized before it renders.
    pub(super) fn html(&self) -> String {
        let mut html = String::with_capacity(self.0.len() * 16);
        for token in &self.0 {
            let escaped = token
                .text
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            if token.kind == TokenKind::Plain {
                html.push_str(&escaped);
            } else {
                html.push_str(&format!(
                    "<span class=\"{}\">{escaped}</span>",
                    token.kind.class_name()
                ));
            }
        }
        html
    }

    fn lex(text: &str, vocabulary: Vocabulary) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let mut tokens: Vec<Token> = Vec::new();
        let mut index = 0;
        while index < chars.len() {
            let start = index;
            let current = chars[index];
            let kind = if current.is_whitespace() {
                while index < chars.len() && chars[index].is_whitespace() {
                    index += 1;
                }
                TokenKind::Plain
            } else if vocabulary == Vocabulary::Sql
                && current == '-'
                && chars.get(index + 1) == Some(&'-')
            {
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
                TokenKind::Comment
            } else if matches!(current, '\'' | '"' | '“') {
                let close = if current == '“' { '”' } else { current };
                index += 1;
                while index < chars.len() {
                    if chars[index] == close {
                        if close == '\'' && chars.get(index + 1) == Some(&'\'') {
                            index += 2;
                            continue;
                        }
                        index += 1;
                        break;
                    }
                    index += 1;
                }
                TokenKind::Literal
            } else if current.is_ascii_digit()
                || (current == '-'
                    && chars.get(index + 1).is_some_and(char::is_ascii_digit)
                    && !Self::binary_context(tokens.last()))
            {
                index += 1;
                while index < chars.len()
                    && (chars[index].is_ascii_alphanumeric() || chars[index] == '.')
                {
                    index += 1;
                }
                TokenKind::Literal
            } else if current.is_alphabetic() || matches!(current, '_' | '$') {
                while index < chars.len()
                    && (chars[index].is_alphanumeric()
                        || matches!(chars[index], '_' | '.' | '$' | '#'))
                {
                    index += 1;
                }
                let word: String = chars[start..index].iter().collect();
                let next = chars[index..].iter().find(|c| !c.is_whitespace());
                if Self::is_keyword(&word, vocabulary) {
                    TokenKind::Keyword
                } else if next == Some(&'(') {
                    TokenKind::Function
                } else {
                    TokenKind::Field
                }
            } else if "=<>!+-*/%|&≠≤≥−×→".contains(current) {
                index += 1;
                while index < chars.len() && "=<>|&".contains(chars[index]) {
                    index += 1;
                }
                TokenKind::Operator
            } else if "(),;:[]{}·".contains(current) {
                index += 1;
                TokenKind::Punct
            } else {
                index += 1;
                TokenKind::Plain
            };
            let text: String = chars[start..index].iter().collect();
            match tokens.last_mut() {
                Some(last) if last.kind == TokenKind::Plain && kind == TokenKind::Plain => {
                    last.text.push_str(&text);
                }
                _ => tokens.push(Token { kind, text }),
            }
        }
        Self(tokens)
    }

    /// A `-` after a value is subtraction; anywhere else it signs a number.
    fn binary_context(previous: Option<&Token>) -> bool {
        let mut previous = previous;
        // Whitespace between the value and the sign does not change the reading.
        if previous.is_some_and(|token| token.kind == TokenKind::Plain) {
            previous = None;
        }
        match previous {
            Some(token) => match token.kind {
                TokenKind::Field | TokenKind::Literal => true,
                TokenKind::Punct => token.text.ends_with(')') || token.text.ends_with(']'),
                _ => false,
            },
            None => false,
        }
    }

    fn is_keyword(word: &str, vocabulary: Vocabulary) -> bool {
        match vocabulary {
            Vocabulary::Sql => {
                let upper = word.to_ascii_uppercase();
                SQL_KEYWORDS.contains(&upper.as_str())
            }
            Vocabulary::Expression => {
                let lower = word.to_ascii_lowercase();
                EXPRESSION_KEYWORDS.contains(&lower.as_str())
            }
        }
    }

    fn render(&self) -> Element {
        rsx! {
            for (index, token) in self.0.iter().enumerate() {
                span { key: "{index}", class: token.kind.class_name(), "{token.text}" }
            }
        }
    }
}

/// SQL text with keywords, functions, strings, and numbers coloured.
#[component]
pub(crate) fn SqlHighlight(sql: String) -> Element {
    let tokens = Tokens::sql(&sql);
    rsx! { code { class: "workflow-code", {tokens.render()} } }
}

/// A rendered graph expression with fields, literals, and operators coloured.
#[component]
pub(super) fn ExpressionHighlight(text: String) -> Element {
    let tokens = Tokens::expression(&text);
    rsx! { span { class: "workflow-expression", {tokens.render()} } }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;

    fn kinds(tokens: &Tokens) -> Vec<(TokenKind, &str)> {
        tokens
            .tokens()
            .iter()
            .filter(|token| token.kind != TokenKind::Plain)
            .map(|token| (token.kind, token.text.as_str()))
            .collect()
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn sql_lexing_reproduces_the_text_and_names_functions_strings_and_comments() {
        let sql = "SELECT spql.ai.score(image, 'a gripper') AS s -- note\nFROM t WHERE x >= -0.5";
        let tokens = Tokens::sql(sql);
        let joined: String = tokens
            .tokens()
            .iter()
            .map(|token| token.text.as_str())
            .collect();
        assert_eq!(joined, sql);
        let coloured = kinds(&tokens);
        assert_eq!(coloured[0], (TokenKind::Keyword, "SELECT"));
        assert_eq!(coloured[1], (TokenKind::Function, "spql.ai.score"));
        assert!(coloured.contains(&(TokenKind::Literal, "'a gripper'")));
        assert!(coloured.contains(&(TokenKind::Comment, "-- note")));
        assert!(coloured.contains(&(TokenKind::Operator, ">=")));
        assert!(coloured.contains(&(TokenKind::Literal, "-0.5")));
        assert!(
            tokens
                .html()
                .contains("<span class=\"tok-keyword\">SELECT</span>")
        );
        assert!(Tokens::sql("a < b").html().contains("&lt;"));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn expression_lexing_separates_fields_from_the_graphs_own_words() {
        let tokens = Tokens::expression(
            "when (holding_object_score − empty_gripper_score ≥ -0.5): \"retained\"; otherwise \"excluded\"",
        );
        let coloured = kinds(&tokens);
        assert_eq!(coloured[0], (TokenKind::Keyword, "when"));
        assert!(coloured.contains(&(TokenKind::Field, "holding_object_score")));
        assert!(coloured.contains(&(TokenKind::Operator, "−")));
        assert!(coloured.contains(&(TokenKind::Literal, "-0.5")));
        assert!(coloured.contains(&(TokenKind::Literal, "\"retained\"")));
        assert!(coloured.contains(&(TokenKind::Keyword, "otherwise")));
        let subtraction = Tokens::expression("a - 1");
        assert!(kinds(&subtraction).contains(&(TokenKind::Operator, "-")));
        let ordering = Tokens::expression("log_time ascending, offset descending");
        assert!(kinds(&ordering).contains(&(TokenKind::Keyword, "ascending")));
        assert!(kinds(&ordering).contains(&(TokenKind::Field, "offset")));
    }
}
