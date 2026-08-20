//! Tests for MPD error handling: ACK format, malformed args, missing args.

use crate::tcp_harness::*;

#[tokio::test]
async fn ack_format_has_code_and_command() {
    let (_server, mut client) = setup().await;
    let resp = client.command("not_a_real_command").await;
    // ACK format: "ACK [error@command_listNum] {current_command} message_text\n"
    assert!(resp.starts_with("ACK ["));
    assert!(resp.contains('{'));
    assert!(resp.contains('}'));
    assert!(resp.ends_with('\n'));
}

#[tokio::test]
async fn ack_for_missing_args() {
    let (_server, mut client) = setup().await;
    // "add" requires a URI argument
    let resp = client.command("add").await;
    assert!(resp.starts_with("ACK "), "missing arg should error: {resp}");
}

#[tokio::test]
async fn ack_for_invalid_volume() {
    let (_server, mut client) = setup().await;
    // Volume must be 0-100
    let resp = client.command("setvol 999").await;
    assert!(
        resp.starts_with("ACK "),
        "invalid volume should error: {resp}"
    );
}

#[tokio::test]
async fn ack_for_non_numeric_argument() {
    let (_server, mut client) = setup().await;
    let resp = client.command("setvol abc").await;
    assert!(
        resp.starts_with("ACK "),
        "non-numeric arg should error: {resp}"
    );
}

#[tokio::test]
async fn ack_for_delete_empty_queue() {
    let (_server, mut client) = setup().await;
    // Position 0 on an empty queue clips to an empty (no-op OK) range per
    // MPD's RangeArg::CheckClip; position 1 is genuinely out of bounds.
    let resp = client.command("delete 1").await;
    assert!(
        resp.starts_with("ACK "),
        "delete on empty queue should error: {resp}"
    );
}

#[tokio::test]
async fn ack_for_play_out_of_range() {
    let (_server, mut client) = setup().await;
    let resp = client.command("play 999").await;
    assert!(
        resp.starts_with("ACK "),
        "play out of range should error: {resp}"
    );
}

#[tokio::test]
async fn ack_for_seekid_nonexistent() {
    let (_server, mut client) = setup().await;
    let resp = client.command("seekid 9999 0").await;
    assert!(
        resp.starts_with("ACK "),
        "seekid non-existent should error: {resp}"
    );
}

#[tokio::test]
async fn ack_for_deleteid_nonexistent() {
    let (_server, mut client) = setup().await;
    let resp = client.command("deleteid 9999").await;
    assert!(
        resp.starts_with("ACK "),
        "deleteid non-existent should error: {resp}"
    );
}

#[tokio::test]
async fn ack_for_unknown_command_has_empty_command_field() {
    let (_server, mut client) = setup().await;
    let resp = client.command("not_a_real_command").await;
    // MPD's `Response::current_command` defaults to "" until a real command
    // is looked up, so an unknown command's `{}` field must stay empty.
    assert_eq!(
        resp,
        "ACK [5@0] {} unknown command \"not_a_real_command\"\n"
    );
}

#[tokio::test]
async fn ack_for_too_few_arguments_names_the_command() {
    let (_server, mut client) = setup().await;
    // "add" needs at least a URI (min=1, max=2 in MPD's commands[] table).
    let resp = client.command("add").await;
    assert_eq!(resp, "ACK [2@0] {add} too few arguments for \"add\"\n");
}

#[tokio::test]
async fn ack_for_too_many_arguments_on_bounded_variable_arity() {
    let (_server, mut client) = setup().await;
    // "add" accepts at most 2 args (URI, position); a 3rd is rejected.
    let resp = client.command("add song.mp3 1 2").await;
    assert_eq!(resp, "ACK [2@0] {add} too many arguments for \"add\"\n");
}

#[tokio::test]
async fn ack_for_wrong_number_on_fixed_arity() {
    let (_server, mut client) = setup().await;
    // "seek" always takes exactly 2 args (position, time); MPD reports any
    // mismatch as "wrong number", never "too few"/"too many".
    let resp = client.command("seek 5").await;
    assert_eq!(
        resp,
        "ACK [2@0] {seek} wrong number of arguments for \"seek\"\n"
    );
}

#[tokio::test]
async fn ack_for_setvol_non_numeric_says_integer_expected() {
    let (_server, mut client) = setup().await;
    let resp = client.command("setvol abc").await;
    assert_eq!(resp, "ACK [2@0] {setvol} Integer expected: abc\n");
}

#[tokio::test]
async fn close_accepts_and_ignores_extra_arguments() {
    let (_server, mut client) = setup().await;
    // MPD's `close` has unchecked arity (min = -1 in commands[]): any
    // trailing arguments are accepted and ignored, not rejected as a bad
    // argument count. It just closes the connection with no response.
    client.send_raw("close extra args\n").await;
    let line = client.read_line().await;
    assert!(
        line.is_empty(),
        "close should close the connection: {line:?}"
    );
}

#[tokio::test]
async fn ack_for_bad_boolean_argument() {
    let (_server, mut client) = setup().await;
    let resp = client.command("pause abc").await;
    assert_eq!(resp, "ACK [2@0] {pause} Boolean (0/1) expected: abc\n");
}

#[tokio::test]
async fn ack_for_bad_float_argument() {
    let (_server, mut client) = setup().await;
    let resp = client.command("seek 0 abc").await;
    assert_eq!(resp, "ACK [2@0] {seek} Float expected: abc\n");
}

#[tokio::test]
async fn ack_for_bad_integer_argument() {
    let (_server, mut client) = setup().await;
    let resp = client.command("addid song.mp3 abc").await;
    assert_eq!(resp, "ACK [2@0] {addid} Integer expected: abc\n");
}

#[tokio::test]
async fn ack_for_out_of_range_priority() {
    let (_server, mut client) = setup().await;
    // `prio`'s priority is bounded to 0-255 (0xff) in MPD's commands[].
    let resp = client.command("prio 300 0").await;
    assert_eq!(resp, "ACK [2@0] {prio} Number too large: 300\n");
}

#[tokio::test]
async fn ack_for_malformed_inverted_range() {
    let (_server, mut client) = setup().await;
    let resp = client.command("move 2:1 0").await;
    assert_eq!(resp, "ACK [2@0] {move} Malformed range: 2:1\n");
}

#[tokio::test]
async fn ack_for_unparseable_range_token() {
    let (_server, mut client) = setup().await;
    let resp = client.command("delete abc").await;
    assert_eq!(resp, "ACK [2@0] {delete} Integer or range expected: abc\n");
}

#[tokio::test]
async fn ack_for_filter_odd_argument_count() {
    let (_server, mut client) = setup().await;
    // A legacy TAG/VALUE filter list with a dangling TAG and no VALUE is a
    // filter-syntax error in MPD's song::Filter, not an arity error, even
    // though `find`'s own min/max (1, unlimited) is satisfied by one token.
    let resp = client.command("find artist").await;
    assert_eq!(
        resp,
        "ACK [2@0] {find} Incorrect number of filter arguments\n"
    );
}

#[tokio::test]
async fn ack_for_embedded_quote_in_unquoted_token() {
    let (_server, mut client) = setup().await;
    // Mirrors `Tokenizer::NextUnquoted`: a literal `'` (or `"`) inside an
    // otherwise-unquoted token is a raw tokenization failure, not an
    // arity/value error — it's thrown (and caught) before the command name
    // is ever looked up, so the `{}` field stays empty.
    let resp = client.command("subscribe 'bad channel'").await;
    assert_eq!(resp, "ACK [5@0] {} Invalid unquoted character\n");
}

#[tokio::test]
async fn ack_for_unterminated_quoted_string() {
    let (_server, mut client) = setup().await;
    let resp = client.command("password \"unterminated").await;
    assert_eq!(resp, "ACK [5@0] {} Missing closing '\"'\n");
}

#[tokio::test]
async fn ack_for_missing_space_after_closing_quote() {
    let (_server, mut client) = setup().await;
    let resp = client.command("password \"closed\"extra").await;
    assert_eq!(resp, "ACK [5@0] {} Space expected after closing '\"'\n");
}

#[tokio::test]
async fn arity_mismatch_takes_precedence_over_a_bad_value() {
    let (_server, mut client) = setup().await;
    // MPD's `command_check_request` (arity) always runs before the
    // handler's own value parsing: 3 args for `seek` (which takes exactly
    // 2) is "wrong number of arguments", not "Float expected: abc", even
    // though the 2nd token is also not a valid float.
    let resp = client.command("seek 0 abc extra").await;
    assert_eq!(
        resp,
        "ACK [2@0] {seek} wrong number of arguments for \"seek\"\n"
    );
}

#[tokio::test]
async fn ack_for_bare_trailing_sort_keyword() {
    let (_server, mut client) = setup().await;
    // MPD's song::Filter only recognizes a trailing "sort"/"window" once a
    // value follows it; a dangling "sort" with nothing after it becomes an
    // unpaired (valueless) filter tag instead.
    let resp = client.command("find artist val sort").await;
    assert_eq!(
        resp,
        "ACK [2@0] {find} Incorrect number of filter arguments\n"
    );
}

#[tokio::test]
async fn ack_for_bare_trailing_position_keyword() {
    let (_server, mut client) = setup().await;
    let resp = client.command("findadd artist val position").await;
    assert_eq!(
        resp,
        "ACK [2@0] {findadd} Incorrect number of filter arguments\n"
    );
}

#[tokio::test]
async fn ack_for_bare_trailing_group_keyword() {
    let (_server, mut client) = setup().await;
    // MPD's `handle_count_internal` only recognizes "group" as the
    // trailing-clause marker when `args[size - 2] == "group"` (i.e. a tag
    // name follows it); a bare trailing "group" with nothing after it
    // falls through to filter parsing as an unpaired filter tag.
    let resp = client.command("searchcount artist val group").await;
    assert_eq!(
        resp,
        "ACK [2@0] {searchcount} Incorrect number of filter arguments\n"
    );
}

#[tokio::test]
async fn ack_for_bare_group_with_no_tag_name() {
    let (_server, mut client) = setup().await;
    let resp = client.command("count group").await;
    assert_eq!(
        resp,
        "ACK [2@0] {count} Incorrect number of filter arguments\n"
    );
}
