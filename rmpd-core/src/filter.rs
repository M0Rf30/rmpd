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
            &input[1..input.len()-1]
        } else {
            input
        };

        Parser::new(input).parse_expression()
    }

    /// Convert filter expression to SQL WHERE clause
    pub fn to_sql(&self) -> (String, Vec<String>) {
        match self {
            FilterExpression::Compare { tag, op, value } => {
                let column = tag_to_column(tag);
                let (sql_op, value_param) = match op {
                    CompareOp::Equal => ("=", value.clone()),
                    CompareOp::NotEqual => ("!=", value.clone()),
                    CompareOp::Regex => {
                        // Convert MPD regex to SQL LIKE pattern
                        // Simple conversion: .* -> %, . -> _, keep others
                        let pattern = value.replace(".*", "%").replace('.', "_");
                        ("LIKE", pattern)
                    }
                    CompareOp::NotRegex => {
                        let pattern = value.replace(".*", "%").replace('.', "_");
                        ("NOT LIKE", pattern)
                    }
                    CompareOp::Less => ("<", value.clone()),
                    CompareOp::Greater => (">", value.clone()),
                    CompareOp::LessEqual => ("<=", value.clone()),
                    CompareOp::GreaterEqual => (">=", value.clone()),
                    CompareOp::Contains => {
                        // contains: substring match -> LIKE '%value%'
                        let pattern = format!("%{}%", value);
                        ("LIKE", pattern)
                    }
                    CompareOp::StartsWith => {
                        // starts_with: prefix match -> LIKE 'value%'
                        let pattern = format!("{}%", value);
                        ("LIKE", pattern)
                    }
                };

                (format!("{} {} ?", column, sql_op), vec![value_param])
            }
            FilterExpression::And(left, right) => {
                let (left_sql, mut left_params) = left.to_sql();
                let (right_sql, right_params) = right.to_sql();
                left_params.extend(right_params);
                (format!("({} AND {})", left_sql, right_sql), left_params)
            }
            FilterExpression::Or(left, right) => {
                let (left_sql, mut left_params) = left.to_sql();
                let (right_sql, right_params) = right.to_sql();
                left_params.extend(right_params);
                (format!("({} OR {})", left_sql, right_sql), left_params)
            }
            FilterExpression::Not(expr) => {
                let (sql, params) = expr.to_sql();
                (format!("NOT ({})", sql), params)
            }
        }
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

    fn parse_expression(&mut self) -> Result<FilterExpression> {
        // Entry point - just parse as OR expression (highest level)
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

        // Check for nested expression
        if self.peek_str("(") {
            let _ = self.consume_str("(");
            let expr = self.parse_or_expression()?;
            self.skip_whitespace();
            self.consume_str(")")?;
            return Ok(expr);
        }

        // Check for NOT
        if self.peek_str("!") {
            let _ = self.consume_str("!");
            self.skip_whitespace();
            let expr = self.parse_primary()?;
            return Ok(FilterExpression::Not(Box::new(expr)));
        }

        // Must be a comparison
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<FilterExpression> {
        self.skip_whitespace();

        // Parse tag name
        let tag = self.parse_identifier()?;
        self.skip_whitespace();

        // Parse operator
        let op = self.parse_operator()?;
        self.skip_whitespace();

        // Parse value (quoted string)
        let value = self.parse_quoted_value()?;

        Ok(FilterExpression::Compare { tag, op, value })
    }

    fn parse_identifier(&mut self) -> Result<String> {
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input.chars().nth(self.pos).unwrap();
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if start == self.pos {
            return Err(RmpdError::ParseError("Expected identifier".to_string()));
        }
        Ok(self.input[start..self.pos].to_string())
    }

    fn parse_operator(&mut self) -> Result<CompareOp> {
        // Try multi-character operators first
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
            Err(RmpdError::ParseError("Expected operator".to_string()))
        }
    }

    fn parse_quoted_value(&mut self) -> Result<String> {
        // Expect single-quoted string
        self.consume_str("'")?;
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input.chars().nth(self.pos).unwrap();
            if ch == '\'' {
                let value = self.input[start..self.pos].to_string();
                self.pos += 1; // consume closing quote
                return Ok(value);
            } else if ch == '\\' && self.pos + 1 < self.input.len() {
                // Skip escaped character
                self.pos += 2;
            } else {
                self.pos += 1;
            }
        }
        Err(RmpdError::ParseError("Unterminated string".to_string()))
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() {
            if self.input.chars().nth(self.pos).unwrap().is_whitespace() {
                self.pos += 1;
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
        // Check that it's followed by whitespace or special char
        if rest.len() == keyword.len() {
            return true;
        }
        let next_ch = rest.chars().nth(keyword.len()).unwrap();
        next_ch.is_whitespace() || next_ch == ')' || next_ch == '('
    }

    fn consume_str(&mut self, s: &str) -> Result<()> {
        if self.input[self.pos..].starts_with(s) {
            self.pos += s.len();
            Ok(())
        } else {
            Err(RmpdError::ParseError(format!("Expected '{}'", s)))
        }
    }
}

/// Convert MPD tag name to database column name
fn tag_to_column(tag: &str) -> &str {
    match tag.to_lowercase().as_str() {
        "artist" => "artist",
        "albumartist" => "album_artist",
        "album" => "album",
        "title" => "title",
        "track" => "track",
        "date" => "date",
        "year" => "date", // year is stored in date column
        "genre" => "genre",
        "composer" => "composer",
        "performer" => "performer",
        "disc" => "disc",
        "comment" => "comment",
        "file" => "path",
        _ => tag, // fallback to tag name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_comparison() {
        let expr = FilterExpression::parse("((Artist == 'Radiohead'))").unwrap();
        let (sql, params) = expr.to_sql();
        assert_eq!(sql, "artist = ?");
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
        let expr = FilterExpression::parse("((artist == 'Radiohead') OR (artist == 'Muse'))").unwrap();
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
}
