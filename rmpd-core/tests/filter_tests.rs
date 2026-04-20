use rmpd_core::filter::{FilterExpression, CompareOp};

#[test]
fn test_simple_equality_filter() {
    let expr = FilterExpression::parse("(artist == 'Radiohead')").unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains("song_tags"));
    assert!(sql.contains("artist"));
    assert_eq!(params, vec!["Radiohead"]);
}

#[test]
fn test_case_insensitive_tag_name() {
    let expr1 = FilterExpression::parse("(Artist == 'Radiohead')").unwrap();
    let expr2 = FilterExpression::parse("(artist == 'Radiohead')").unwrap();
    
    let (sql1, params1) = expr1.to_sql();
    let (sql2, params2) = expr2.to_sql();
    
    // Both should generate similar SQL (tag name should be lowercase)
    assert!(sql1.contains("artist"));
    assert!(sql2.contains("artist"));
    assert_eq!(params1, params2);
}

#[test]
fn test_and_combination() {
    let expr = FilterExpression::parse("((artist == 'Radiohead') AND (genre == 'Rock'))").unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains("AND"));
    assert_eq!(params.len(), 2);
    assert!(params.contains(&"Radiohead".to_string()));
    assert!(params.contains(&"Rock".to_string()));
}

#[test]
fn test_or_combination() {
    let expr = FilterExpression::parse("((artist == 'Radiohead') OR (artist == 'Muse'))").unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains("OR"));
    assert_eq!(params.len(), 2);
    assert!(params.contains(&"Radiohead".to_string()));
    assert!(params.contains(&"Muse".to_string()));
}

#[test]
fn test_negation() {
    let expr = FilterExpression::parse("(!(genre == 'Pop'))").unwrap();
    let (sql, _) = expr.to_sql();
    
    assert!(sql.contains("NOT"));
}

#[test]
fn test_not_equal_operator() {
    let expr = FilterExpression::parse("(artist != 'Unknown')").unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains("!="));
    assert_eq!(params, vec!["Unknown"]);
}

#[test]
fn test_regex_operator() {
    let expr = FilterExpression::parse("(artist =~ 'Radio.*')").unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains("LIKE"));
    assert_eq!(params, vec!["Radio%"]);
}

#[test]
fn test_not_regex_operator() {
    let expr = FilterExpression::parse("(artist !~ 'Unknown.*')").unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains("NOT LIKE"));
    assert_eq!(params, vec!["Unknown%"]);
}

#[test]
fn test_less_than_operator() {
    let expr = FilterExpression::parse("(date < '2000')").unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains("<"));
    assert_eq!(params, vec!["2000"]);
}

#[test]
fn test_greater_than_operator() {
    let expr = FilterExpression::parse("(date > '2000')").unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains(">"));
    assert_eq!(params, vec!["2000"]);
}

#[test]
fn test_less_equal_operator() {
    let expr = FilterExpression::parse("(date <= '2000')").unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains("<="));
    assert_eq!(params, vec!["2000"]);
}

#[test]
fn test_greater_equal_operator() {
    let expr = FilterExpression::parse("(date >= '2000')").unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains(">="));
    assert_eq!(params, vec!["2000"]);
}

#[test]
fn test_file_tag_special_handling() {
    let expr = FilterExpression::parse("(file == 'some/path.mp3')").unwrap();
    let (sql, params) = expr.to_sql();
    
    // file tag should map to path column directly
    assert_eq!(sql, "path = ?");
    assert_eq!(params, vec!["some/path.mp3"]);
}

#[test]
fn test_albumartist_fallback_chain() {
    let expr = FilterExpression::parse("(AlbumArtist == 'Led Zeppelin')").unwrap();
    let (sql, params) = expr.to_sql();
    
    // Should generate IN clause for fallback tags
    assert!(sql.contains("IN"));
    assert!(sql.contains("albumartist"));
    assert!(sql.contains("artist"));
    assert_eq!(params.len(), 1);
    assert_eq!(params[0], "Led Zeppelin");
}

#[test]
fn test_double_quoted_values() {
    let expr = FilterExpression::parse("(artist == \"Amon Tobin\")").unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains("song_tags"));
    assert_eq!(params, vec!["Amon Tobin"]);
}

#[test]
fn test_escaped_quotes_in_values() {
    let expr = FilterExpression::parse(r#"(artist == "Guns \"N\" Roses")"#).unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains("song_tags"));
    assert_eq!(params, vec![r#"Guns "N" Roses"#]);
}

#[test]
fn test_complex_nested_expression() {
    let expr = FilterExpression::parse(
        "((artist == 'Radiohead') AND ((genre == 'Rock') OR (genre == 'Alternative')))"
    ).unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains("AND"));
    assert!(sql.contains("OR"));
    assert_eq!(params.len(), 3);
}

#[test]
fn test_contains_operator() {
    let expr = FilterExpression::parse("(title contains 'love')").unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains("LIKE"));
    assert_eq!(params, vec!["%love%"]);
}

#[test]
fn test_starts_with_operator() {
    let expr = FilterExpression::parse("(title starts_with 'The')").unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains("LIKE"));
    assert_eq!(params, vec!["The%"]);
}

#[test]
fn test_filter_expression_equality() {
    let expr1 = FilterExpression::Compare {
        tag: "artist".to_string(),
        op: CompareOp::Equal,
        value: "Radiohead".to_string(),
    };
    let expr2 = FilterExpression::Compare {
        tag: "artist".to_string(),
        op: CompareOp::Equal,
        value: "Radiohead".to_string(),
    };
    
    assert_eq!(expr1, expr2);
}

#[test]
fn test_multiple_and_operators() {
    let expr = FilterExpression::parse(
        "((artist == 'Radiohead') AND (genre == 'Rock') AND (date >= '1990'))"
    ).unwrap();
    let (sql, params) = expr.to_sql();
    
    assert!(sql.contains("AND"));
    assert_eq!(params.len(), 3);
}
