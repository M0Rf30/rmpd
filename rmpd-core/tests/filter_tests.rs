use rmpd_core::filter::{CompareOp, FilterExpression};

#[test]
fn test_simple_equality_filter() {
    let expr = FilterExpression::parse("(artist == 'Radiohead')", false).unwrap();
    let (sql, params) = expr.to_sql();

    assert!(sql.contains("song_tags"));
    assert_eq!(params, vec!["artist", "Radiohead"]);
}

#[test]
fn test_case_insensitive_tag_name() {
    let expr1 = FilterExpression::parse("(Artist == 'Radiohead')", false).unwrap();
    let expr2 = FilterExpression::parse("(artist == 'Radiohead')", false).unwrap();

    let (sql1, params1) = expr1.to_sql();
    let (sql2, params2) = expr2.to_sql();

    // Both should generate identical SQL and params (tag name normalized to lowercase)
    assert_eq!(sql1, sql2);
    assert_eq!(params1, params2);
    assert_eq!(params1, vec!["artist", "Radiohead"]);
}

#[test]
fn test_find_vs_search_case_sensitivity() {
    // find (fold_case=false) is case-sensitive: plain `=`, no COLLATE.
    let find_expr = FilterExpression::parse("(artist == 'Radiohead')", false).unwrap();
    let (find_sql, _) = find_expr.to_sql();
    assert!(
        !find_sql.contains("COLLATE"),
        "find should be case-sensitive: {find_sql}"
    );

    // search (fold_case=true) is case-insensitive: `= ? COLLATE NOCASE`.
    let search_expr = FilterExpression::parse("(artist == 'Radiohead')", true).unwrap();
    let (search_sql, _) = search_expr.to_sql();
    assert!(
        search_sql.contains("COLLATE NOCASE"),
        "search should be case-insensitive: {search_sql}"
    );
}

#[test]
fn test_and_combination() {
    let expr =
        FilterExpression::parse("((artist == 'Radiohead') AND (genre == 'Rock'))", false).unwrap();
    let (sql, params) = expr.to_sql();

    assert!(sql.contains("AND"));
    assert_eq!(params.len(), 4);
    assert!(params.contains(&"artist".to_string()));
    assert!(params.contains(&"Radiohead".to_string()));
    assert!(params.contains(&"genre".to_string()));
    assert!(params.contains(&"Rock".to_string()));
}

#[test]
fn test_or_is_rejected() {
    // MPD's grammar (Filter.cxx::ParseExpression) has no `OR` — only `AND`
    // chains of parenthesized sub-expressions.
    let err = FilterExpression::parse("((artist == 'Radiohead') OR (artist == 'Muse'))", false)
        .unwrap_err();
    assert_eq!(err.to_string(), "Parse error: 'AND' expected");
}

#[test]
fn test_negation() {
    let expr = FilterExpression::parse("(!(genre == 'Pop'))", false).unwrap();
    let (sql, _) = expr.to_sql();

    assert!(sql.contains("NOT"));
}

#[test]
fn test_not_equal_operator() {
    let expr = FilterExpression::parse("(artist != 'Unknown')", false).unwrap();
    let (sql, params) = expr.to_sql();

    // `!=` is `Equal` + `negated`: the whole EXISTS is NOT-wrapped rather
    // than emitting a literal `!=` SQL operator.
    assert!(sql.starts_with("NOT ("), "got: {sql}");
    assert_eq!(params, vec!["artist", "Unknown"]);
}

#[test]
fn test_regex_operator() {
    let expr = FilterExpression::parse("(artist =~ 'Radio.*')", false).unwrap();
    let (sql, params) = expr.to_sql();

    assert!(sql.contains("REGEXP"));
    assert_eq!(params, vec!["artist", "Radio.*"]);
}

#[test]
fn test_not_regex_operator() {
    let expr = FilterExpression::parse("(artist !~ 'Unknown.*')", false).unwrap();
    let (sql, params) = expr.to_sql();

    assert!(sql.starts_with("NOT ("), "got: {sql}");
    assert!(sql.contains("REGEXP"), "got: {sql}");
    assert_eq!(params, vec!["artist", "Unknown.*"]);
}

// MPD's `operators` table (Filter.cxx:209) is 12 string operators plus
// `==`/`!=`/`=~`/`!~`/`contains`/`starts_with` — no relational operators on
// arbitrary tags exist at all (the only numeric comparison in the whole
// grammar is the hardcoded `prio >= N`). `<`/`>`/`<=`/`>=` must be rejected
// with MPD's exact "Unknown filter operator: <remainder>" text, not parsed.

#[test]
fn test_less_than_operator_is_rejected() {
    let err = FilterExpression::parse("(date < '2000')", false).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Parse error: Unknown filter operator: < '2000')"
    );
}

#[test]
fn test_greater_than_operator_is_rejected() {
    let err = FilterExpression::parse("(date > '2000')", false).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Parse error: Unknown filter operator: > '2000')"
    );
}

#[test]
fn test_less_equal_operator_is_rejected() {
    let err = FilterExpression::parse("(date <= '2000')", false).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Parse error: Unknown filter operator: <= '2000')"
    );
}

#[test]
fn test_greater_equal_operator_is_rejected() {
    let err = FilterExpression::parse("(date >= '2000')", false).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Parse error: Unknown filter operator: >= '2000')"
    );
}

#[test]
fn test_file_tag_special_handling() {
    let expr = FilterExpression::parse("(file == 'some/path.mp3')", false).unwrap();
    let (sql, params) = expr.to_sql();

    // file tag should map to path column directly
    assert_eq!(sql, "path = ?");
    assert_eq!(params, vec!["some/path.mp3"]);
}

#[test]
fn test_albumartist_fallback_chain() {
    let expr = FilterExpression::parse("(AlbumArtist == 'Led Zeppelin')", false).unwrap();
    let (sql, params) = expr.to_sql();

    // Should generate IN clause for fallback tags
    assert!(sql.contains("IN"));
    assert_eq!(params.len(), 3);
    assert_eq!(params[0], "albumartist");
    assert_eq!(params[1], "artist");
    assert_eq!(params[2], "Led Zeppelin");
}

#[test]
fn test_double_quoted_values() {
    let expr = FilterExpression::parse("(artist == \"Amon Tobin\")", false).unwrap();
    let (sql, params) = expr.to_sql();

    assert!(sql.contains("song_tags"));
    assert_eq!(params, vec!["artist", "Amon Tobin"]);
}

#[test]
fn test_escaped_quotes_in_values() {
    let expr = FilterExpression::parse(r#"(artist == "Guns \"N\" Roses")"#, false).unwrap();
    let (sql, params) = expr.to_sql();

    assert!(sql.contains("song_tags"));
    assert_eq!(params, vec!["artist", r#"Guns "N" Roses"#]);
}

#[test]
fn test_complex_nested_expression() {
    // MPD's grammar has no OR; deep nesting is still exercised via AND,
    // nested rather than flat (each AND member is itself a parenthesized
    // AND-group here, not a bare comparison).
    let expr = FilterExpression::parse(
        "((artist == 'Radiohead') AND ((genre == 'Rock') AND (date == '1990')))",
        false,
    )
    .unwrap();
    let (sql, params) = expr.to_sql();

    // 2 structural ANDs from the nesting, plus 2 per comparison (one from
    // the tag_restriction's own `AND`, one joining it to the value
    // comparison) — 3 comparisons here.
    assert_eq!(sql.matches("AND").count(), 8);
    assert_eq!(params.len(), 6);
}

#[test]
fn test_contains_operator() {
    // fold_case=true (search semantics): bare `contains` without an
    // explicit `_cs`/`_ci` suffix uses the command's default case
    // sensitivity, which is what selects the LIKE-based (case-insensitive)
    // SQL form asserted below.
    let expr = FilterExpression::parse("(title contains 'love')", true).unwrap();
    let (sql, params) = expr.to_sql();

    assert!(sql.contains("LIKE"));
    assert_eq!(params, vec!["title", "%love%"]);
}

#[test]
fn test_starts_with_operator() {
    let expr = FilterExpression::parse("(title starts_with 'The')", true).unwrap();
    let (sql, params) = expr.to_sql();

    assert!(sql.contains("LIKE"));
    assert_eq!(params, vec!["title", "The%"]);
}

#[test]
fn test_filter_expression_equality() {
    let expr1 = FilterExpression::Compare {
        tag: "artist".to_string(),
        op: CompareOp::Equal,
        value: "Radiohead".to_string(),
        case_sensitive: true,
        negated: false,
    };
    let expr2 = FilterExpression::Compare {
        tag: "artist".to_string(),
        op: CompareOp::Equal,
        value: "Radiohead".to_string(),
        case_sensitive: true,
        negated: false,
    };

    assert_eq!(expr1, expr2);
}

#[test]
fn test_contains_escapes_percent() {
    let expr = FilterExpression::parse("(artist contains '50%')", true).unwrap();
    let (sql, params) = expr.to_sql();

    assert!(sql.contains("LIKE"));
    assert!(sql.contains("ESCAPE '\\'"), "got: {sql}");
    assert_eq!(params, vec!["artist", "%50\\%%"]);
}

#[test]
fn test_starts_with_escapes_underscore() {
    let expr = FilterExpression::parse("(title starts_with 'foo_bar')", true).unwrap();
    let (sql, params) = expr.to_sql();

    assert!(sql.contains("LIKE"));
    assert!(sql.contains("ESCAPE '\\'"), "got: {sql}");
    assert_eq!(params, vec!["title", "foo\\_bar%"]);
}

#[test]
fn test_contains_escapes_backslash() {
    let expr = FilterExpression::parse(r"(title contains 'a\\b')", true).unwrap();
    let (sql, params) = expr.to_sql();

    assert!(sql.contains("ESCAPE '\\'"), "got: {sql}");
    assert_eq!(params, vec!["title", "%a\\\\b%"]);
}

#[test]
fn test_non_like_operators_have_no_escape_clause() {
    let expr = FilterExpression::parse("(artist == '50%')", false).unwrap();
    let (sql, params) = expr.to_sql();

    assert!(!sql.contains("ESCAPE"), "got: {sql}");
    assert_eq!(params, vec!["artist", "50%"]);
}

#[test]
fn test_explicit_cs_ci_operator_suffixes() {
    // `eq_cs`/`eq_ci` etc. override the command's default case sensitivity
    // regardless of `fold_case`.
    let cs_under_search = FilterExpression::parse("(Artist eq_cs 'X')", true).unwrap();
    let (sql, _) = cs_under_search.to_sql();
    assert!(
        !sql.contains("COLLATE NOCASE"),
        "eq_cs must stay case-sensitive: {sql}"
    );

    let ci_under_find = FilterExpression::parse("(Artist eq_ci 'X')", false).unwrap();
    let (sql, _) = ci_under_find.to_sql();
    assert!(
        sql.contains("COLLATE NOCASE"),
        "eq_ci must stay case-insensitive: {sql}"
    );

    let cs_contains = FilterExpression::parse("(Artist contains_cs 'X')", true).unwrap();
    let (sql, params) = cs_contains.to_sql();
    assert!(sql.contains("INSTR"), "contains_cs should use INSTR: {sql}");
    assert_eq!(params, vec!["artist", "X"]);
}

#[test]
fn test_any_tag_sentinel() {
    // The special `any` tag checks all tag types: no `st.tag IN (...)`
    // restriction at all.
    let expr = FilterExpression::parse("(any == 'X')", false).unwrap();
    let (sql, params) = expr.to_sql();
    assert!(
        !sql.contains("st.tag IN"),
        "any must not restrict tag: {sql}"
    );
    assert_eq!(params, vec!["X"]);
}

#[test]
fn test_base_filter() {
    let expr = FilterExpression::parse("(base 'Music/Rock')", false).unwrap();
    let (sql, params) = expr.to_sql();
    assert_eq!(sql, "(path = ? OR path LIKE ? ESCAPE '\\')");
    assert_eq!(params, vec!["Music/Rock", "Music/Rock/%"]);
}

#[test]
fn test_modified_since_filter() {
    let expr = FilterExpression::parse("(modified-since '2023-01-15T10:30:00Z')", false).unwrap();
    let (sql, params) = expr.to_sql();
    assert_eq!(sql, "last_modified >= ?");
    assert_eq!(params, vec!["1673778600"]);
}

#[test]
fn test_added_since_filter() {
    let expr = FilterExpression::parse("(added-since '1700000000')", false).unwrap();
    let (sql, params) = expr.to_sql();
    assert_eq!(sql, "added_at >= ?");
    assert_eq!(params, vec!["1700000000"]);
}

#[test]
fn test_audio_format_filter() {
    let exact = FilterExpression::parse("(AudioFormat == '44100:16:2')", false).unwrap();
    let (sql, params) = exact.to_sql();
    assert!(sql.contains("sample_rate = ?"));
    assert!(sql.contains("bits_per_sample = ?"));
    assert!(sql.contains("channels = ?"));
    assert_eq!(params, vec!["44100", "16", "2"]);

    let mask = FilterExpression::parse("(AudioFormat =~ '*:16:2')", false).unwrap();
    let (sql, params) = mask.to_sql();
    assert!(!sql.contains("sample_rate = ?"));
    assert_eq!(params, vec!["16", "2"]);
}

#[test]
fn test_priority_filter() {
    let zero = FilterExpression::parse("(prio >= 0)", false).unwrap();
    let (sql, _) = zero.to_sql();
    assert_eq!(sql, "1");

    let nonzero = FilterExpression::parse("(prio >= 5)", false).unwrap();
    let (sql, _) = nonzero.to_sql();
    assert_eq!(sql, "0");
}
