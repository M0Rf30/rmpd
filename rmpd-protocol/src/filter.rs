/// MPD Filter Expression Parser
///
/// Implements the MPD filter syntax for find/search commands.
///
/// Syntax:
/// - Basic: (TAG OPERATOR 'VALUE')
/// - Operators: ==, !=, contains, starts_with, =~, !~
/// - Boolean: (EXPR AND EXPR), (!EXPR)
/// - Nested: ((TAG == 'A') AND (TAG2 == 'B'))
use winnow::prelude::*;
use winnow::token::{take_till, take_while};
use winnow::ascii::{space0, space1};

#[derive(Debug, Clone, PartialEq)]
pub enum FilterExpr {
    /// Simple comparison: (tag op value)
    Compare {
        tag: String,
        operator: FilterOp,
        value: String,
    },
    /// Boolean AND: (expr AND expr)
    And(Vec<FilterExpr>),
    /// Boolean NOT: (!expr)
    Not(Box<FilterExpr>),
    /// Raw expression (fallback for complex/unsupported syntax)
    Raw(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FilterOp {
    /// == (equals)
    Eq,
    /// != (not equals)
    Ne,
    /// contains (substring match)
    Contains,
    /// starts_with (prefix match)
    StartsWith,
    /// =~ (regex match)
    Regex,
    /// !~ (negated regex)
    NotRegex,
    /// Case-sensitive variants
    EqCs,
    EqCi,
    ContainsCs,
    ContainsCi,
    StartsWithCs,
    StartsWithCi,
    /// Comparison operators for numeric values
    Gt,
    Gte,
    Lt,
    Lte,
}

impl FilterOp {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "==" => Some(Self::Eq),
            "!=" => Some(Self::Ne),
            "contains" => Some(Self::Contains),
            "starts_with" => Some(Self::StartsWith),
            "=~" => Some(Self::Regex),
            "!~" => Some(Self::NotRegex),
            "eq_cs" => Some(Self::EqCs),
            "eq_ci" => Some(Self::EqCi),
            "contains_cs" => Some(Self::ContainsCs),
            "contains_ci" => Some(Self::ContainsCi),
            "starts_with_cs" => Some(Self::StartsWithCs),
            "starts_with_ci" => Some(Self::StartsWithCi),
            ">" => Some(Self::Gt),
            ">=" => Some(Self::Gte),
            "<" => Some(Self::Lt),
            "<=" => Some(Self::Lte),
            _ => None,
        }
    }
}

/// Parse a complete filter expression
pub fn parse_filter(input: &str) -> Result<FilterExpr, String> {
    filter_expr.parse(input.trim()).map_err(|e| e.to_string())
}

fn filter_expr(input: &mut &str) -> winnow::ModalResult<FilterExpr> {
    let _ = space0.parse_next(input)?;

    if input.starts_with('(') {
        parse_parenthesized_expr(input)
    } else {
        // Fallback: treat entire input as raw expression
        let raw = input.to_string();
        *input = "";
        Ok(FilterExpr::Raw(raw))
    }
}

fn parse_parenthesized_expr(input: &mut &str) -> winnow::ModalResult<FilterExpr> {
    let _ = '('.parse_next(input)?;
    let _ = space0.parse_next(input)?;

    // Check for negation
    if input.starts_with('!') {
        let _ = '!'.parse_next(input)?;
        let _ = space0.parse_next(input)?;
        let inner = parse_parenthesized_expr(input)?;
        let _ = space0.parse_next(input)?;
        let _ = ')'.parse_next(input)?;
        return Ok(FilterExpr::Not(Box::new(inner)));
    }

    // Try to parse as comparison or AND expression
    // First, try to parse a tag
    let first_token = parse_token(input)?;
    let _ = space1.parse_next(input)?;

    // Check if this is an AND expression
    if first_token.starts_with('(') {
        // This is a nested expression followed by AND
        // Reset and parse properly
        *input = &input[(first_token.len())..];

        let mut exprs = vec![parse_parenthesized_expr(input)?];

        loop {
            let _ = space0.parse_next(input)?;

            // Check for AND
            if input.starts_with("AND") {
                let _ = "AND".parse_next(input)?;
                let _ = space1.parse_next(input)?;
                exprs.push(parse_parenthesized_expr(input)?);
            } else {
                break;
            }
        }

        let _ = space0.parse_next(input)?;
        let _ = ')'.parse_next(input)?;

        if exprs.len() == 1 {
            return Ok(exprs.into_iter().next().unwrap());
        } else {
            return Ok(FilterExpr::And(exprs));
        }
    }

    // Parse as comparison: TAG OP VALUE
    let tag = first_token;
    let op_str = parse_token(input)?;
    let _ = space1.parse_next(input)?;
    let value = parse_quoted_value(input)?;
    let _ = space0.parse_next(input)?;

    // Check for AND after this comparison
    if input.starts_with("AND") {
        let first_operator = match FilterOp::from_str(&op_str) {
            Some(op) => op,
            None => return Ok(FilterExpr::Raw(format!("({} {} {})", tag, op_str, value))),
        };

        let mut exprs = vec![FilterExpr::Compare {
            tag,
            operator: first_operator,
            value,
        }];

        while input.starts_with("AND") {
            let _ = "AND".parse_next(input)?;
            let _ = space1.parse_next(input)?;

            // Parse next expression
            if input.starts_with('(') {
                exprs.push(parse_parenthesized_expr(input)?);
            } else {
                // Parse another comparison
                let next_tag = parse_token(input)?;
                let _ = space1.parse_next(input)?;
                let next_op = parse_token(input)?;
                let _ = space1.parse_next(input)?;
                let next_val = parse_quoted_value(input)?;

                let next_operator = match FilterOp::from_str(&next_op) {
                    Some(op) => op,
                    None => return Ok(FilterExpr::Raw(format!("({} {} {})", next_tag, next_op, next_val))),
                };

                exprs.push(FilterExpr::Compare {
                    tag: next_tag,
                    operator: next_operator,
                    value: next_val,
                });
            }

            let _ = space0.parse_next(input)?;
        }

        let _ = ')'.parse_next(input)?;
        Ok(FilterExpr::And(exprs))
    } else {
        let _ = ')'.parse_next(input)?;

        let operator = match FilterOp::from_str(&op_str) {
            Some(op) => op,
            None => return Ok(FilterExpr::Raw(format!("({} {} {})", tag, op_str, value))),
        };

        Ok(FilterExpr::Compare { tag, operator, value })
    }
}

fn parse_token(input: &mut &str) -> winnow::ModalResult<String> {
    let token = take_while(1.., |c: char| {
        !c.is_whitespace() && c != ')' && c != '('
    }).parse_next(input)?;
    Ok(token.to_string())
}

fn parse_quoted_value(input: &mut &str) -> winnow::ModalResult<String> {
    if input.starts_with('\'') {
        // Single-quoted string
        let _ = '\''.parse_next(input)?;
        let value = take_till(1.., '\'').parse_next(input)?;
        let _ = '\''.parse_next(input)?;
        Ok(value.to_string())
    } else if input.starts_with('"') {
        // Double-quoted string
        let _ = '"'.parse_next(input)?;
        let value = take_till(1.., '"').parse_next(input)?;
        let _ = '"'.parse_next(input)?;
        Ok(value.to_string())
    } else {
        // Unquoted token
        parse_token(input)
    }
}

/// Convert filter expression back to tag/value pairs for backward compatibility
pub fn filter_to_legacy_pairs(expr: &FilterExpr) -> Vec<(String, String)> {
    match expr {
        FilterExpr::Compare { tag, operator: _, value } => {
            // For simple comparisons, just return the tag/value pair
            vec![(tag.clone(), value.clone())]
        }
        FilterExpr::And(exprs) => {
            // Flatten all comparisons
            exprs.iter()
                .flat_map(filter_to_legacy_pairs)
                .collect()
        }
        FilterExpr::Not(inner) => {
            // For negated expressions, try to convert the inner expression
            filter_to_legacy_pairs(inner)
        }
        FilterExpr::Raw(s) => {
            // Raw expression: try to extract something useful
            vec![("raw_filter".to_string(), s.clone())]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_comparison() {
        let expr = parse_filter("(Artist == 'Madonna')").unwrap();
        match expr {
            FilterExpr::Compare { tag, operator, value } => {
                assert_eq!(tag, "Artist");
                assert_eq!(operator, FilterOp::Eq);
                assert_eq!(value, "Madonna");
            }
            _ => panic!("Expected Compare expression"),
        }
    }

    #[test]
    fn test_and_expression() {
        let expr = parse_filter("((Artist == 'Madonna') AND (Album == 'Ray of Light'))").unwrap();
        match expr {
            FilterExpr::And(exprs) => {
                assert_eq!(exprs.len(), 2);
            }
            _ => panic!("Expected And expression"),
        }
    }

    #[test]
    fn test_not_expression() {
        let expr = parse_filter("(!(Artist == 'Madonna'))").unwrap();
        match expr {
            FilterExpr::Not(inner) => {
                match *inner {
                    FilterExpr::Compare { tag, .. } => {
                        assert_eq!(tag, "Artist");
                    }
                    _ => panic!("Expected Compare inside Not"),
                }
            }
            _ => panic!("Expected Not expression"),
        }
    }

    #[test]
    fn test_contains_operator() {
        let expr = parse_filter("(Title contains 'love')").unwrap();
        match expr {
            FilterExpr::Compare { tag, operator, value } => {
                assert_eq!(tag, "Title");
                assert_eq!(operator, FilterOp::Contains);
                assert_eq!(value, "love");
            }
            _ => panic!("Expected Compare expression"),
        }
    }
}
