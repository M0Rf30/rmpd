/// MPD filter expression parsing and evaluation
/// Implements the filter syntax introduced in MPD 0.21+
///
/// Grammar:
/// EXPRESSION := (EXPRESSION)
///            | (EXPRESSION) AND (EXPRESSION)
///            | (EXPRESSION) OR (EXPRESSION)
///            | ! EXPRESSION
///            | TAG OPERATOR VALUE
/// OPERATOR := == | != | =~ | !~ | < | > | <= | >=
use crate::error::{Result, RmpdError};
use crate::tag::tag_fallback_chain;

#[derive(Debug, Clone, PartialEq)]
pub enum FilterExpression {
    /// Tag comparison: tag, operator, value
    Compare {
        tag: String,
        op: CompareOp,
        value: String,
    },
    /// Logical AND
    And(Box<FilterExpression>, Box<FilterExpression>),
    /// Logical OR
    Or(Box<FilterExpression>, Box<FilterExpression>),
    /// Logical NOT
    Not(Box<FilterExpression>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Equal,        // ==
    NotEqual,     // !=
    Regex,        // =~
    NotRegex,     // !~
    Less,         // <
    Greater,      // >
    LessEqual,    // <=
    GreaterEqual, // >=
    Contains,     // contains
    StartsWith,   // starts_with
}

impl FilterExpression {
    /// Parse a filter expression string
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();

        // Remove outer parentheses if present
        let input = if input.starts_with('(') && input.ends_with(')') {
            &input[1..input.len() - 1]
        } else {
            input
        };

        Parser::new(input).parse_expression()
    }

    /// Convert filter expression to SQL WHERE clause using EXISTS subqueries on song_tags.
    /// The songs table is referenced as `songs` (no alias).
    pub fn to_sql(&self) -> (String, Vec<String>) {
        match self {
            FilterExpression::Compare { tag, op, value } => {
                let tag_lower = tag.to_lowercase();

                // `file` tag matches against the path column directly
                if tag_lower == "file" {
                    let (sql_op, value_param) = op_to_sql(op, value);
                    return (format!("path {sql_op} ?"), vec![value_param]);
                }

                // For tags with fallback chains, generate OR-ed EXISTS subqueries
                let fallback_tags = tag_fallback_chain(&tag_lower);
                let (sql_op, value_param) = op_to_sql(op, value);

                if fallback_tags.len() == 1 {
                    let sql = format!(
                        "EXISTS (SELECT 1 FROM song_tags st WHERE st.song_id = songs.id AND st.tag = '{}' AND st.value {sql_op} ?)",
                        fallback_tags[0]
                    );
                    (sql, vec![value_param])
                } else {
                    // Multiple fallback tags: EXISTS for first tag OR (NOT EXISTS first AND EXISTS second) etc.
                    // Simplified: just OR the EXISTS clauses (matches MPD behavior for filter matching)
                    let clauses: Vec<String> = fallback_tags
                        .iter()
                        .map(|t| {
                            format!(
                                "EXISTS (SELECT 1 FROM song_tags st WHERE st.song_id = songs.id AND st.tag = '{t}' AND st.value {sql_op} ?)"
                            )
                        })
                        .collect();
                    let sql = format!("({})", clauses.join(" OR "));
                    let params = vec![value_param; fallback_tags.len()];
                    (sql, params)
                }
            }
            FilterExpression::And(left, right) => {
                let (left_sql, mut left_params) = left.to_sql();
                let (right_sql, right_params) = right.to_sql();
                left_params.extend(right_params);
                (format!("({left_sql} AND {right_sql})"), left_params)
            }
            FilterExpression::Or(left, right) => {
                let (left_sql, mut left_params) = left.to_sql();
                let (right_sql, right_params) = right.to_sql();
                left_params.extend(right_params);
                (format!("({left_sql} OR {right_sql})"), left_params)
            }
            FilterExpression::Not(expr) => {
                let (sql, params) = expr.to_sql();
                (format!("NOT ({sql})"), params)
            }
        }
    }
}

fn op_to_sql(op: &CompareOp, value: &str) -> (&'static str, String) {
    match op {
        CompareOp::Equal => ("=", value.to_string()),
        CompareOp::NotEqual => ("!=", value.to_string()),
        CompareOp::Regex => ("LIKE", value.replace(".*", "%").replace('.', "_")),
        CompareOp::NotRegex => ("NOT LIKE", value.replace(".*", "%").replace('.', "_")),
        CompareOp::Less => ("<", value.to_string()),
        CompareOp::Greater => (">", value.to_string()),
        CompareOp::LessEqual => ("<=", value.to_string()),
        CompareOp::GreaterEqual => (">=", value.to_string()),
        CompareOp::Contains => ("LIKE", format!("%{value}%")),
        CompareOp::StartsWith => ("LIKE", format!("{value}%")),
    }
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn peek_char(&self) -> Result<char> {
        self.input.chars().nth(self.pos).ok_or_else(|| {
            RmpdError::ParseError(format!("Unexpected end of input at position {}", self.pos))
        })
    }

    fn parse_expression(&mut self) -> Result<FilterExpression> {
        self.parse_or_expression()
    }

    fn parse_or_expression(&mut self) -> Result<FilterExpression> {
        let mut left = self.parse_and_expression()?;

        self.skip_whitespace();
        while self.peek_keyword("OR") {
            self.consume_str("OR")?;
            self.skip_whitespace();
            let right = self.parse_and_expression()?;
            left = FilterExpression::Or(Box::new(left), Box::new(right));
            self.skip_whitespace();
        }

        Ok(left)
    }

    fn parse_and_expression(&mut self) -> Result<FilterExpression> {
        let mut left = self.parse_primary()?;

        self.skip_whitespace();
        while self.peek_keyword("AND") {
            self.consume_str("AND")?;
            self.skip_whitespace();
            let right = self.parse_primary()?;
            left = FilterExpression::And(Box::new(left), Box::new(right));
            self.skip_whitespace();
        }

        Ok(left)
    }

    fn parse_primary(&mut self) -> Result<FilterExpression> {
        self.skip_whitespace();

        if self.peek_str("(") {
            let _ = self.consume_str("(");
            let expr = self.parse_or_expression()?;
            self.skip_whitespace();
            self.consume_str(")")?;
            return Ok(expr);
        }

        if self.peek_str("!") {
            let _ = self.consume_str("!");
            self.skip_whitespace();
            let expr = self.parse_primary()?;
            return Ok(FilterExpression::Not(Box::new(expr)));
        }

        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<FilterExpression> {
        self.skip_whitespace();

        let tag = self.parse_identifier()?;
        self.skip_whitespace();

        let op = self.parse_operator()?;
        self.skip_whitespace();

        let value = self.parse_quoted_value()?;

        Ok(FilterExpression::Compare { tag, op, value })
    }

    fn parse_identifier(&mut self) -> Result<String> {
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.peek_char()?;
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if start == self.pos {
            return Err(RmpdError::ParseError("Expected identifier".to_owned()));
        }
        Ok(self.input[start..self.pos].to_string())
    }

    fn parse_operator(&mut self) -> Result<CompareOp> {
        if self.consume_str("starts_with").is_ok() {
            Ok(CompareOp::StartsWith)
        } else if self.consume_str("contains").is_ok() {
            Ok(CompareOp::Contains)
        } else if self.consume_str("==").is_ok() {
            Ok(CompareOp::Equal)
        } else if self.consume_str("!=").is_ok() {
            Ok(CompareOp::NotEqual)
        } else if self.consume_str("=~").is_ok() {
            Ok(CompareOp::Regex)
        } else if self.consume_str("!~").is_ok() {
            Ok(CompareOp::NotRegex)
        } else if self.consume_str("<=").is_ok() {
            Ok(CompareOp::LessEqual)
        } else if self.consume_str(">=").is_ok() {
            Ok(CompareOp::GreaterEqual)
        } else if self.consume_str("<").is_ok() {
            Ok(CompareOp::Less)
        } else if self.consume_str(">").is_ok() {
            Ok(CompareOp::Greater)
        } else {
            Err(RmpdError::ParseError("Expected operator".to_owned()))
        }
    }

    fn parse_quoted_value(&mut self) -> Result<String> {
        let quote_char = self.peek_char()?;
        if quote_char != '\'' && quote_char != '"' {
            return Err(RmpdError::ParseError("Quoted string expected".to_owned()));
        }
        self.pos += 1;

        let mut result = String::new();
        while self.pos < self.input.len() {
            let ch = self.peek_char()?;
            if ch == quote_char {
                self.pos += 1;
                return Ok(result);
            } else if ch == '\\' && self.pos + 1 < self.input.len() {
                self.pos += 1;
                let escaped = self.peek_char()?;
                result.push(escaped);
                self.pos += 1;
            } else {
                result.push(ch);
                self.pos += ch.len_utf8();
            }
        }
        Err(RmpdError::ParseError("Unterminated string".to_owned()))
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() {
            if let Ok(ch) = self.peek_char() {
                if ch.is_whitespace() {
                    self.pos += 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }

    fn peek_str(&self, s: &str) -> bool {
        self.input[self.pos..].starts_with(s)
    }

    fn peek_keyword(&self, keyword: &str) -> bool {
        let rest = &self.input[self.pos..];
        if !rest.starts_with(keyword) {
            return false;
        }
        if rest.len() == keyword.len() {
            return true;
        }
        if let Some(next_ch) = rest.chars().nth(keyword.len()) {
            next_ch.is_whitespace() || next_ch == ')' || next_ch == '('
        } else {
            false
        }
    }

    fn consume_str(&mut self, s: &str) -> Result<()> {
        if self.input[self.pos..].starts_with(s) {
            self.pos += s.len();
            Ok(())
        } else {
            Err(RmpdError::ParseError(format!("Expected '{s}'")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_comparison() {
        let expr = FilterExpression::parse("((Artist == 'Radiohead'))").unwrap();
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("song_tags"));
        assert!(sql.contains("artist"));
        assert_eq!(params, vec!["Radiohead"]);
    }

    #[test]
    fn test_and_expression() {
        let expr = FilterExpression::parse("((date >= '2000') AND (genre == 'Rock'))").unwrap();
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("AND"), "SQL should contain AND: {}", sql);
        assert_eq!(params, vec!["2000", "Rock"]);
    }

    #[test]
    fn test_or_expression() {
        let expr =
            FilterExpression::parse("((artist == 'Radiohead') OR (artist == 'Muse'))").unwrap();
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("OR"), "SQL should contain OR: {}", sql);
        assert_eq!(params, vec!["Radiohead", "Muse"]);
    }

    #[test]
    fn test_not_expression() {
        let expr = FilterExpression::parse("(!(genre == 'Pop'))").unwrap();
        let (sql, _) = expr.to_sql();
        assert!(sql.contains("NOT"));
    }

    #[test]
    fn test_regex() {
        let expr = FilterExpression::parse("((Artist =~ 'Radio.*'))").unwrap();
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("LIKE"));
        assert_eq!(params, vec!["Radio%"]);
    }

    #[test]
    fn test_double_quoted_values() {
        let expr = FilterExpression::parse("(Artist == \"Amon Tobin\")").unwrap();
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("song_tags"));
        assert_eq!(params, vec!["Amon Tobin"]);
    }

    #[test]
    fn test_double_quoted_with_escape() {
        let expr = FilterExpression::parse(r#"(Artist == "Guns \"N\" Roses")"#).unwrap();
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("song_tags"));
        assert_eq!(params, vec![r#"Guns "N" Roses"#]);
    }

    #[test]
    fn test_albumartist_fallback() {
        let expr = FilterExpression::parse("(AlbumArtist == 'Led Zeppelin')").unwrap();
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("albumartist"));
        assert!(sql.contains("artist"));
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn test_file_tag() {
        let expr = FilterExpression::parse("(file == 'some/path.mp3')").unwrap();
        let (sql, params) = expr.to_sql();
        assert_eq!(sql, "path = ?");
        assert_eq!(params, vec!["some/path.mp3"]);
    }
}
