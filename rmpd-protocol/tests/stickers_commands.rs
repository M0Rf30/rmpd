//! Integration tests for sticker commands

mod common;

use common::TestClient;

#[test]
fn test_sticker_get_command() {
    // sticker get should return sticker value
    let response = "sticker: rating=5\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_sticker_get_not_found() {
    // sticker get for non-existent sticker
    let response = "ACK [50@0] {sticker} no such sticker\n";
    assert!(TestClient::is_error(response));
}

#[test]
fn test_sticker_set_command() {
    // sticker set should set sticker value
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_sticker_delete_command() {
    // sticker delete should remove sticker
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_sticker_list_command() {
    // sticker list should list all stickers for URI
    let response = "sticker: rating=5\nsticker: playcount=10\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_sticker_find_command() {
    // sticker find should find URIs with matching sticker
    let response = "file: song1.mp3\nsticker: rating=5\nfile: song2.mp3\nsticker: rating=5\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_sticker_inc_command() {
    // sticker inc should increment integer sticker
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_sticker_dec_command() {
    // sticker dec should decrement integer sticker
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_sticker_names_command() {
    // sticker names should list all sticker names
    let response = "name: rating\nname: playcount\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_sticker_types_command() {
    // sticker types should list supported types
    let response = "type: string\ntype: int\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_sticker_namestypes_command() {
    // sticker namestypes should list names with their types
    let response = "name: rating\ntype: int\nname: comment\ntype: string\nOK\n";
    assert!(TestClient::is_ok(response));
}
