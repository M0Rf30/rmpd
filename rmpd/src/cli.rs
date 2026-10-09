// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! One-shot CLI actions: `rmpd deps` and `rmpd --kill`.

use anyhow::{Context, Result, anyhow, bail};
use rmpd_core::config::NetworkConfig;
use std::io::{BufRead, BufReader, Read, Write};

/// Cargo features this binary was built with.
fn enabled_features() -> Vec<&'static str> {
    let all: &[(&str, bool)] = &[
        ("pipewire", cfg!(feature = "pipewire")),
        ("subsonic", cfg!(feature = "subsonic")),
        ("jellyfin", cfg!(feature = "jellyfin")),
        ("podcast", cfg!(feature = "podcast")),
        ("radio", cfg!(feature = "radio")),
        ("alsa-mixer", cfg!(feature = "alsa-mixer")),
        ("listenbrainz", cfg!(feature = "listenbrainz")),
        ("lastfm", cfg!(feature = "lastfm")),
        ("webhook", cfg!(feature = "webhook")),
        ("http-api", cfg!(feature = "http-api")),
    ];
    all.iter().filter(|(_, on)| *on).map(|(n, _)| *n).collect()
}

fn line(label: &str, items: &[String]) {
    println!("  {label:<18} {}", items.join(", "));
}

/// Print version, build info and every compiled-in plugin (Mopidy `deps`).
pub fn print_deps() {
    println!("rmpd {}", env!("CARGO_PKG_VERSION"));
    println!(
        "  {:<18} {} / {}",
        "platform",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let features = enabled_features();
    line(
        "features",
        &[if features.is_empty() {
            "(none)".to_owned()
        } else {
            features.join(", ")
        }],
    );
    println!("plugins:");
    line(
        "outputs",
        &rmpd_player::output_registry::OUTPUT_PLUGINS
            .iter()
            .map(|(n, _)| (*n).to_owned())
            .collect::<Vec<_>>(),
    );
    line(
        "mixers",
        &rmpd_player::mixer::MIXER_PLUGINS
            .iter()
            .map(|p| p.name.to_owned())
            .collect::<Vec<_>>(),
    );
    line(
        "encoders",
        &rmpd_player::encoder::encoder_names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>(),
    );
    line(
        "decoders",
        &rmpd_player::format_registry::FORMAT_PLUGINS
            .iter()
            .map(|f| f.plugin.to_owned())
            .collect::<Vec<_>>(),
    );
    line(
        "inputs",
        &rmpd_stream::INPUT_PLUGINS
            .iter()
            .map(|p: &&dyn rmpd_stream::InputPlugin| {
                format!("{} ({})", p.name(), p.schemes().join("/"))
            })
            .collect::<Vec<_>>(),
    );
    line(
        "playlists",
        &rmpd_plugin::playlist::PLAYLIST_PLUGINS
            .iter()
            .map(|p| p.name().to_owned())
            .collect::<Vec<_>>(),
    );
    line(
        "sources",
        &rmpd_source::SOURCE_PLUGINS
            .iter()
            .map(|p| p.name.to_owned())
            .collect::<Vec<_>>(),
    );
    line(
        "integrations",
        &rmpd_integrations::INTEGRATION_PLUGINS
            .iter()
            .map(|p| p.name.to_owned())
            .collect::<Vec<_>>(),
    );
    line(
        "artwork",
        &rmpd_integrations::ARTWORK_PLUGINS
            .iter()
            .map(|p| p.name.to_owned())
            .collect::<Vec<_>>(),
    );
}

/// Ask a running rmpd to shut down over the MPD protocol (MPD's `--kill`).
pub fn kill_running(net: &NetworkConfig) -> Result<()> {
    let password = net.password.clone().or_else(|| {
        net.passwords
            .iter()
            .find(|p| p.permissions.iter().any(|x| x == "admin"))
            .map(|p| p.password.clone())
    });

    #[cfg(unix)]
    {
        let socket = if net.bind_address.starts_with('/') {
            Some(std::path::PathBuf::from(&net.bind_address))
        } else {
            net.unix_socket
                .as_ref()
                .map(|p| p.as_std_path().to_path_buf())
        };
        if let Some(path) = socket
            && let Ok(stream) = std::os::unix::net::UnixStream::connect(&path)
        {
            return send_kill(stream, password.as_deref());
        }
    }

    if net.bind_address.is_empty() || net.bind_address.starts_with('/') {
        bail!("no reachable rmpd socket (is rmpd running?)");
    }
    let host = match net.bind_address.as_str() {
        "0.0.0.0" | "any" => "127.0.0.1",
        "::" => "::1",
        h => h,
    };
    let addr = rmpd_protocol::server::resolve_bind_address(host, net.port);
    let stream = std::net::TcpStream::connect(&addr)
        .with_context(|| format!("cannot connect to rmpd at {addr} (is it running?)"))?;
    send_kill(stream, password.as_deref())
}

fn send_kill<S: Read + Write>(stream: S, password: Option<&str>) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut greeting = String::new();
    reader.read_line(&mut greeting)?;
    if !greeting.starts_with("OK MPD") {
        bail!("unexpected greeting: {}", greeting.trim());
    }
    if let Some(pw) = password {
        writeln!(reader.get_mut(), "password \"{}\"", pw.replace('"', "\\\""))?;
        let mut reply = String::new();
        reader.read_line(&mut reply)?;
        if !reply.starts_with("OK") {
            return Err(anyhow!("authentication failed"));
        }
    }
    writeln!(reader.get_mut(), "kill")?;
    reader.get_mut().flush()?;
    // The server closes the connection on success; an ACK means refused.
    let mut reply = String::new();
    let _ = reader.read_line(&mut reply);
    if reply.starts_with("ACK") {
        bail!("rmpd refused: {}", reply.trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }
    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buf)
        }
    }
    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.output.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn kill_sends_password_then_kill() {
        let mut d = Duplex {
            input: Cursor::new(b"OK MPD 0.24.0\nOK\n".to_vec()),
            output: Vec::new(),
        };
        send_kill(&mut d, Some("pw")).unwrap();
        assert_eq!(
            String::from_utf8(d.output).unwrap(),
            "password \"pw\"\nkill\n"
        );
    }

    #[test]
    fn kill_reports_refusal() {
        let d = Duplex {
            input: Cursor::new(
                b"OK MPD 0.24.0\nACK [4@0] {kill} you don't have permission\n".to_vec(),
            ),
            output: Vec::new(),
        };
        assert!(send_kill(d, None).is_err());
    }
}
