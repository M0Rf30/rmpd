// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Mapping of rmpd bus events to Mopidy core event names/payloads
//! (`track_playback_started`, `playback_state_changed`, `volume_changed`,
//! `tracklist_changed`, `options_changed`, `seeked`, `stream_title_changed`,
//! plus `track_playback_paused/resumed/ended`).
//!
//! [`EventMapper`] is pure and synchronous; [`run_pump`] feeds it from the
//! event bus and fans the resulting JSON messages out to WebSocket clients.

use super::model::{state_name, tl_track_json};
use rmpd_core::event::Event;
use rmpd_core::song::Song;
use rmpd_core::state::PlayerState;
use rmpd_plugin::integration::{PlayerHandle, ShutdownSignal};
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast;

/// A position jump larger than this (beyond normal playback progress) is
/// reported as `seeked`.
const SEEK_TOLERANCE_MS: u64 = 1500;

/// One Mopidy event: its name and keyword payload.
#[derive(Debug, Clone, PartialEq)]
pub struct MopidyEvent {
    pub name: &'static str,
    /// JSON object with the event's keyword arguments.
    pub data: Value,
}

impl MopidyEvent {
    fn new(name: &'static str, data: Value) -> Self {
        Self { name, data }
    }

    /// Wire form pushed to clients: `{"event": <name>, ...kwargs}`.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut map = Map::new();
        map.insert("event".to_owned(), json!(self.name));
        if let Value::Object(data) = &self.data {
            for (k, v) in data {
                map.insert(k.clone(), v.clone());
            }
        }
        Value::Object(map).to_string()
    }
}

/// Player facts the mapper cannot get from the event itself.
#[derive(Debug, Clone, Default)]
pub struct EventContext {
    /// Queue id of the current song.
    pub tlid: Option<u32>,
    /// Current song.
    pub song: Option<Arc<Song>>,
    /// Live playback position in milliseconds.
    pub position_ms: u64,
}

impl EventContext {
    fn tl_track(&self) -> Option<Value> {
        match (self.tlid, &self.song) {
            (Some(id), Some(song)) => Some(tl_track_json(id, song)),
            _ => None,
        }
    }
}

fn millis(d: std::time::Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Stateful event translator (remembers the playback state to report
/// `old_state`, the announced track, and the last position to detect seeks).
#[derive(Debug)]
pub struct EventMapper {
    state: PlayerState,
    tl_track: Option<Value>,
    song_announced: bool,
    /// `(position_ms, observed_at_ms)`.
    last_position: Option<(u64, u64)>,
}

impl EventMapper {
    #[must_use]
    pub fn new(initial: PlayerState) -> Self {
        Self {
            state: initial,
            tl_track: None,
            song_announced: false,
            last_position: None,
        }
    }

    /// Whether [`map`](Self::map) needs a fresh [`EventContext`] for `event`.
    #[must_use]
    pub fn needs_context(event: &Event) -> bool {
        matches!(event, Event::SongChanged(_) | Event::PlayerStateChanged(_))
    }

    /// Translate one bus event. `now_ms` is a monotonic millisecond clock.
    pub fn map(&mut self, event: &Event, ctx: &EventContext, now_ms: u64) -> Vec<MopidyEvent> {
        match event {
            Event::SongChanged(song) => self.song_changed(song.as_ref(), ctx, now_ms),
            Event::PlayerStateChanged(new) => self.state_changed(*new, ctx, now_ms),
            Event::PositionChanged(d) => self.position_changed(millis(*d), now_ms),
            Event::VolumeChanged(v) => {
                vec![MopidyEvent::new("volume_changed", json!({ "volume": v }))]
            }
            Event::QueueChanged => vec![MopidyEvent::new("tracklist_changed", json!({}))],
            Event::QueueOptionsChanged => vec![MopidyEvent::new("options_changed", json!({}))],
            Event::StreamTitleChanged(title) => vec![MopidyEvent::new(
                "stream_title_changed",
                json!({ "title": title.clone().unwrap_or_default() }),
            )],
            _ => Vec::new(),
        }
    }

    fn song_changed(
        &mut self,
        song: Option<&Song>,
        ctx: &EventContext,
        now_ms: u64,
    ) -> Vec<MopidyEvent> {
        let mut out = Vec::new();
        let Some(song) = song else {
            self.tl_track = None;
            self.song_announced = false;
            return out;
        };
        let was_active = matches!(self.state, PlayerState::Play | PlayerState::Pause);
        if was_active
            && self.song_announced
            && let Some(old) = self.tl_track.clone()
        {
            let at = self.last_position.map_or(ctx.position_ms, |p| p.0);
            out.push(MopidyEvent::new(
                "track_playback_ended",
                json!({ "tl_track": old, "time_position": at }),
            ));
        }
        let tl_track = tl_track_json(ctx.tlid.unwrap_or(0), song);
        self.tl_track = Some(tl_track.clone());
        self.song_announced = true;
        self.last_position = Some((0, now_ms));
        out.push(MopidyEvent::new(
            "track_playback_started",
            json!({ "tl_track": tl_track }),
        ));
        out
    }

    fn state_changed(
        &mut self,
        new: PlayerState,
        ctx: &EventContext,
        now_ms: u64,
    ) -> Vec<MopidyEvent> {
        let old = self.state;
        if old == new {
            return Vec::new();
        }
        self.state = new;
        let mut out = vec![MopidyEvent::new(
            "playback_state_changed",
            json!({ "old_state": state_name(old), "new_state": state_name(new) }),
        )];
        let known = self.tl_track.clone().or_else(|| ctx.tl_track());
        match (old, new) {
            (PlayerState::Play, PlayerState::Pause) => {
                self.last_position = Some((ctx.position_ms, now_ms));
                if let Some(tl_track) = known {
                    out.push(MopidyEvent::new(
                        "track_playback_paused",
                        json!({ "tl_track": tl_track, "time_position": ctx.position_ms }),
                    ));
                }
            }
            (PlayerState::Pause, PlayerState::Play) => {
                self.last_position = Some((ctx.position_ms, now_ms));
                if let Some(tl_track) = known {
                    out.push(MopidyEvent::new(
                        "track_playback_resumed",
                        json!({ "tl_track": tl_track, "time_position": ctx.position_ms }),
                    ));
                }
            }
            (PlayerState::Stop, PlayerState::Play) => {
                self.last_position = Some((ctx.position_ms, now_ms));
                if !self.song_announced
                    && let Some(tl_track) = known
                {
                    self.tl_track = Some(tl_track.clone());
                    self.song_announced = true;
                    out.push(MopidyEvent::new(
                        "track_playback_started",
                        json!({ "tl_track": tl_track }),
                    ));
                }
            }
            (_, PlayerState::Stop) => {
                let at = self.last_position.map_or(ctx.position_ms, |p| p.0);
                self.last_position = None;
                self.song_announced = false;
                if let Some(tl_track) = known {
                    out.push(MopidyEvent::new(
                        "track_playback_ended",
                        json!({ "tl_track": tl_track, "time_position": at }),
                    ));
                }
            }
            _ => {}
        }
        out
    }

    fn position_changed(&mut self, position_ms: u64, now_ms: u64) -> Vec<MopidyEvent> {
        let mut out = Vec::new();
        if let Some((last_ms, seen_at)) = self.last_position {
            let expected = if self.state == PlayerState::Play {
                last_ms.saturating_add(now_ms.saturating_sub(seen_at))
            } else {
                last_ms
            };
            if position_ms.abs_diff(expected) > SEEK_TOLERANCE_MS {
                out.push(MopidyEvent::new(
                    "seeked",
                    json!({ "time_position": position_ms }),
                ));
            }
        }
        if self.state != PlayerState::Stop {
            self.last_position = Some((position_ms, now_ms));
        }
        out
    }
}

/// Fetch the player facts [`EventMapper::map`] needs for `event`.
pub async fn context_for(player: &dyn PlayerHandle, event: &Event) -> EventContext {
    if !EventMapper::needs_context(event) {
        return EventContext::default();
    }
    let snapshot = player.status().await;
    EventContext {
        tlid: player.current_song_id().await,
        song: snapshot.song,
        position_ms: player
            .position()
            .await
            .or(snapshot.elapsed)
            .map_or(0, millis),
    }
}

/// Translate bus events until shutdown and broadcast the JSON messages.
/// Lagging receivers skip ahead; a closed bus ends the loop.
pub async fn run_pump(
    mut events: broadcast::Receiver<Event>,
    player: Arc<dyn PlayerHandle>,
    out: broadcast::Sender<Arc<str>>,
    mut shutdown: ShutdownSignal,
) {
    let clock = Instant::now();
    let mut mapper = EventMapper::new(player.status().await.state);
    loop {
        let event = tokio::select! {
            () = shutdown.cancelled() => return,
            received = events.recv() => match received {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            },
        };
        let ctx = context_for(player.as_ref(), &event).await;
        let now_ms = millis(clock.elapsed());
        for mapped in mapper.map(&event, &ctx, now_ms) {
            // No subscribers is fine: nobody is listening.
            let _ = out.send(Arc::from(mapped.to_json()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_api::testutil::song;
    use std::time::Duration;

    fn names(events: &[MopidyEvent]) -> Vec<&'static str> {
        events.iter().map(|e| e.name).collect()
    }

    fn ctx(tlid: u32, position_ms: u64) -> EventContext {
        EventContext {
            tlid: Some(tlid),
            song: Some(Arc::new(song("a.flac", &[("title", "First")]))),
            position_ms,
        }
    }

    #[test]
    fn stateless_events_map_directly() {
        let mut m = EventMapper::new(PlayerState::Stop);
        let c = EventContext::default();
        let v = m.map(&Event::VolumeChanged(35), &c, 0);
        assert_eq!(names(&v), ["volume_changed"]);
        assert_eq!(v[0].data["volume"], 35);
        assert_eq!(
            names(&m.map(&Event::QueueChanged, &c, 0)),
            ["tracklist_changed"]
        );
        assert_eq!(
            names(&m.map(&Event::QueueOptionsChanged, &c, 0)),
            ["options_changed"]
        );
        let t = m.map(&Event::StreamTitleChanged(Some("Radio X".into())), &c, 0);
        assert_eq!(names(&t), ["stream_title_changed"]);
        assert_eq!(t[0].data["title"], "Radio X");
        let cleared = m.map(&Event::StreamTitleChanged(None), &c, 0);
        assert_eq!(cleared[0].data["title"], "");
        assert!(m.map(&Event::SongFinished, &c, 0).is_empty());
    }

    #[test]
    fn wire_form_flattens_payload() {
        let e = MopidyEvent::new("volume_changed", json!({ "volume": 5 }));
        let v: Value = serde_json::from_str(&e.to_json()).unwrap();
        assert_eq!(v["event"], "volume_changed");
        assert_eq!(v["volume"], 5);
    }

    #[test]
    fn song_change_announces_start_then_end_of_previous() {
        let mut m = EventMapper::new(PlayerState::Play);
        let first = m.map(
            &Event::SongChanged(Some(song("a.flac", &[]))),
            &ctx(10, 0),
            0,
        );
        assert_eq!(names(&first), ["track_playback_started"]);
        assert_eq!(first[0].data["tl_track"]["tlid"], 10);
        let second = m.map(
            &Event::SongChanged(Some(song("b.flac", &[]))),
            &ctx(11, 0),
            30_000,
        );
        assert_eq!(
            names(&second),
            ["track_playback_ended", "track_playback_started"]
        );
        assert_eq!(second[0].data["tl_track"]["tlid"], 10);
        assert_eq!(second[1].data["tl_track"]["tlid"], 11);
    }

    #[test]
    fn state_transitions_emit_mopidy_events() {
        let mut m = EventMapper::new(PlayerState::Stop);
        let c = ctx(10, 4000);
        let start = m.map(&Event::PlayerStateChanged(PlayerState::Play), &c, 0);
        assert_eq!(
            names(&start),
            ["playback_state_changed", "track_playback_started"]
        );
        assert_eq!(start[0].data["old_state"], "stopped");
        assert_eq!(start[0].data["new_state"], "playing");
        let pause = m.map(&Event::PlayerStateChanged(PlayerState::Pause), &c, 10);
        assert_eq!(
            names(&pause),
            ["playback_state_changed", "track_playback_paused"]
        );
        assert_eq!(pause[1].data["time_position"], 4000);
        let resume = m.map(&Event::PlayerStateChanged(PlayerState::Play), &c, 20);
        assert_eq!(
            names(&resume),
            ["playback_state_changed", "track_playback_resumed"]
        );
        let stop = m.map(&Event::PlayerStateChanged(PlayerState::Stop), &c, 30);
        assert_eq!(
            names(&stop),
            ["playback_state_changed", "track_playback_ended"]
        );
        assert_eq!(stop[0].data["new_state"], "stopped");
        // Same state again is not a change.
        assert!(
            m.map(&Event::PlayerStateChanged(PlayerState::Stop), &c, 40)
                .is_empty()
        );
    }

    #[test]
    fn song_changed_before_state_change_is_not_announced_twice() {
        let mut m = EventMapper::new(PlayerState::Stop);
        let c = ctx(10, 0);
        let a = m.map(&Event::SongChanged(Some(song("a.flac", &[]))), &c, 0);
        assert_eq!(names(&a), ["track_playback_started"]);
        let b = m.map(&Event::PlayerStateChanged(PlayerState::Play), &c, 1);
        assert_eq!(names(&b), ["playback_state_changed"]);
    }

    #[test]
    fn position_jump_is_a_seek() {
        let mut m = EventMapper::new(PlayerState::Play);
        let c = ctx(10, 0);
        m.map(&Event::SongChanged(Some(song("a.flac", &[]))), &c, 0);
        // Normal progress: ~1s later at 1s.
        assert!(
            m.map(
                &Event::PositionChanged(Duration::from_millis(1000)),
                &c,
                1000
            )
            .is_empty()
        );
        // Small jitter is fine.
        assert!(
            m.map(
                &Event::PositionChanged(Duration::from_millis(2100)),
                &c,
                2000
            )
            .is_empty()
        );
        // Jump forward by a minute.
        let seeked = m.map(
            &Event::PositionChanged(Duration::from_millis(62_000)),
            &c,
            3000,
        );
        assert_eq!(names(&seeked), ["seeked"]);
        assert_eq!(seeked[0].data["time_position"], 62_000);
        // Progress continues from the new position without another seek.
        assert!(
            m.map(
                &Event::PositionChanged(Duration::from_millis(63_000)),
                &c,
                4000
            )
            .is_empty()
        );
    }

    #[test]
    fn needs_context_only_for_song_and_state() {
        assert!(EventMapper::needs_context(&Event::SongChanged(None)));
        assert!(EventMapper::needs_context(&Event::PlayerStateChanged(
            PlayerState::Play
        )));
        assert!(!EventMapper::needs_context(&Event::VolumeChanged(1)));
    }

    #[tokio::test]
    async fn pump_broadcasts_mapped_events() {
        use crate::http_api::testutil::MockPlayer;
        use rmpd_plugin::integration::shutdown_channel;

        let player: Arc<dyn PlayerHandle> = Arc::new(MockPlayer::with_queue());
        let (bus_tx, bus_rx) = broadcast::channel::<Event>(16);
        let (out_tx, mut out_rx) = broadcast::channel::<Arc<str>>(16);
        let (trigger, signal) = shutdown_channel();
        let task = tokio::spawn(run_pump(bus_rx, player, out_tx, signal));
        bus_tx.send(Event::VolumeChanged(12)).unwrap();
        let msg = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["event"], "volume_changed");
        assert_eq!(v["volume"], 12);
        trigger.trigger();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
    }
}
