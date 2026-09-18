use crate::artwork::{infer_mime, picture_type_to_string};
use camino::Utf8PathBuf;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::{Song, intern_tag_key};
use rmpd_core::tag::{normalize_decimal, vorbis_tag_map_get};
use rmpd_core::time::system_time_to_unix_secs;
use std::borrow::Cow;
use std::fs;
use std::time::{Duration, SystemTime};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, MediaInfo, Track, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{
    Metadata, MetadataOptions, MetadataRevision, RawValue, StandardTag, Tag, Visual,
};
use symphonia::core::units::Duration as SymDuration;

fn is_bogus_dsf_comment(s: &str) -> bool {
    let trimmed = s.trim();
    trimmed.len() >= 16 && trimmed.chars().all(|c| c.is_ascii_hexdigit() || c == ' ')
}

#[derive(Debug, Clone)]
pub struct Artwork {
    pub picture_type: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

/// Result of a single `symphonia` probe pass: the container-native tags, embedded pictures,
/// and audio properties derived from the default audio track.
struct Probed {
    tags: Vec<Tag>,
    visuals: Vec<Artwork>,
    duration: Option<Duration>,
    sample_rate: Option<u32>,
    channels: Option<u8>,
    bit_depth: Option<u8>,
    bitrate: Option<u32>,
}

/// Open `path` once, probe it with `symphonia`, and collect everything callers need: the
/// container-native tags, embedded pictures (only when `want_visuals` is set -- collecting
/// picture bytes is wasted work for callers that only read tags), and audio properties. Used
/// by `extract_from_file`, `extract_artwork_from_file`, and `read_raw_comments` so none of
/// them re-open or re-probe the file.
fn probe_file(path: &Utf8PathBuf, want_visuals: bool) -> Result<Probed> {
    let file = fs::File::open(path.as_str())
        .map_err(|e| RmpdError::Library(format!("Failed to open file: {e}")))?;
    let file_size = file.metadata().map(|m| m.len()).unwrap_or(0);

    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        hint.with_extension(ext);
    }

    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut reader = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| RmpdError::Library(format!("Failed to probe format: {e}")))?;

    let (sample_rate, channels, bit_depth, duration, track_id) = {
        let track = reader.default_track(TrackType::Audio);
        let audio = track.and_then(|t| match t.codec_params.as_ref() {
            Some(CodecParameters::Audio(a)) => Some(a),
            _ => None,
        });
        let sample_rate = audio.and_then(|a| a.sample_rate);
        let channels = audio.and_then(|a| a.channels.as_ref().map(|c| c.count() as u8));
        let bit_depth = audio.and_then(|a| a.bits_per_sample).map(|b| b as u8);
        let duration = track_duration(track, reader.media_info());
        let track_id = track.map(|t| u64::from(t.id));
        (sample_rate, channels, bit_depth, duration, track_id)
    };

    let (tags, visuals, visual_bytes) =
        drain_metadata(&mut reader.metadata(), want_visuals, track_id);

    // Symphonia exposes no bitrate field; derive kbps from the audio-bitstream portion of the
    // file (excluding embedded artwork, which is not part of the audio stream) and duration,
    // matching the unit lofty reported.
    let bitrate = duration.filter(|d| d.as_secs_f64() > 0.0).map(|d| {
        let audio_bytes = file_size.saturating_sub(visual_bytes);
        ((audio_bytes as f64 * 8.0) / d.as_secs_f64() / 1000.0) as u32
    });

    Ok(Probed {
        tags,
        visuals,
        duration,
        sample_rate,
        channels,
        bit_depth,
        bitrate,
    })
}

/// Compute a track's duration, falling back through every level of precision symphonia
/// exposes: the track's own frame count, then its declared duration in timebase units (set by
/// e.g. isomp4 when frame count isn't derivable), then the reader's overall media duration
/// (the only value MKV/WebM ever populate, since that demuxer never sets per-track duration).
fn track_duration(track: Option<&Track>, media_info: &MediaInfo) -> Option<Duration> {
    let from_track = track.and_then(|t| {
        let tb = t.time_base?;
        let dur = match t.num_frames {
            Some(n) => SymDuration::from(n),
            None => t.duration?,
        };
        Some((tb, dur))
    });

    let (tb, dur) = from_track.or_else(|| Some((media_info.time_base?, media_info.duration?)))?;

    // `calc_duration` operates on an unsigned tick count, so unlike a `Timestamp`-based
    // calculation there is no signed-overflow/panic risk from an oversized or corrupt frame
    // count; `try_from_secs_f64` additionally guards against a NaN/negative result.
    tb.calc_duration(dur)
        .and_then(|time| Duration::try_from_secs_f64(time.as_secs_f64()).ok())
}

/// Drain the metadata log into `(tags, visuals, visual_byte_total)`.
///
/// `tags` comes from exactly one revision: the newest one that actually has tags (checking the
/// default audio track's per-track tags too, since Matroska routes track-targeted tag elements
/// there instead of to the media-level tag list). Symphonia probes standalone leading/trailing
/// tag readers (e.g. ID3v1) before the container's own reader runs, so merging every revision
/// would let a stale/truncated tag block (ID3v1's 30-char fields) shadow, or duplicate, the
/// real one.
///
/// `visual_byte_total` is summed from every revision regardless of `want_visuals`, since it
/// only reads picture lengths (no copying); `visuals` itself is only populated -- cloning
/// picture bytes -- when a caller actually wants pictures.
fn drain_metadata(
    log: &mut Metadata<'_>,
    want_visuals: bool,
    track_id: Option<u64>,
) -> (Vec<Tag>, Vec<Artwork>, u64) {
    let mut older: Vec<MetadataRevision> = Vec::new();
    while let Some(discarded) = log.pop() {
        older.push(discarded);
    }

    let mut visual_bytes: u64 = older.iter().map(revision_visual_bytes).sum();
    if let Some(rev) = log.current() {
        visual_bytes += revision_visual_bytes(rev);
    }

    let tags = match log.current() {
        Some(rev) if revision_has_tags(rev, track_id) => revision_tags(rev, track_id),
        _ => older
            .iter()
            .rev()
            .find(|r| revision_has_tags(r, track_id))
            .map(|r| revision_tags(r, track_id))
            .unwrap_or_default(),
    };

    let mut visuals = Vec::new();
    if want_visuals {
        if let Some(rev) = log.current() {
            push_visuals(rev.media.visuals.clone(), &mut visuals);
        }
        for rev in older {
            push_visuals(rev.media.visuals, &mut visuals);
        }
    }

    (tags, visuals, visual_bytes)
}

fn revision_visual_bytes(rev: &MetadataRevision) -> u64 {
    rev.media.visuals.iter().map(|v| v.data.len() as u64).sum()
}

fn revision_has_tags(rev: &MetadataRevision, track_id: Option<u64>) -> bool {
    !rev.media.tags.is_empty()
        || track_id.is_some_and(|id| {
            rev.per_track
                .iter()
                .any(|pt| pt.track_id == id && !pt.metadata.tags.is_empty())
        })
}

fn revision_tags(rev: &MetadataRevision, track_id: Option<u64>) -> Vec<Tag> {
    let mut tags = rev.media.tags.clone();
    if let Some(id) = track_id {
        for pt in &rev.per_track {
            if pt.track_id == id {
                tags.extend(pt.metadata.tags.iter().cloned());
            }
        }
    }
    tags
}

fn push_visuals(visuals: Vec<Visual>, out: &mut Vec<Artwork>) {
    for v in visuals {
        let mime_type = v
            .media_type
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| infer_mime(&v.data).to_owned());
        out.push(Artwork {
            picture_type: picture_type_to_string(v.usage),
            mime_type,
            data: Vec::from(v.data),
        });
    }
}

/// Map the `StandardTag` variants that translate 1:1 into an MPD tag name and string value.
/// MusicBrainz and label/publisher tags land here because symphonia already merges their
/// Vorbis-key, ID3 TXXX-description, and MP4 freeform-atom spellings into one variant.
fn simple_std_tag(std: &StandardTag) -> Option<(&'static str, &str)> {
    match std {
        StandardTag::TrackTitle(v) => Some(("title", v.as_str())),
        StandardTag::Artist(v) => Some(("artist", v.as_str())),
        StandardTag::Album(v) => Some(("album", v.as_str())),
        StandardTag::AlbumArtist(v) => Some(("albumartist", v.as_str())),
        StandardTag::Genre(v) => Some(("genre", v.as_str())),
        StandardTag::Composer(v) => Some(("composer", v.as_str())),
        StandardTag::Performer(v) => Some(("performer", v.as_str())),
        StandardTag::Grouping(v) => Some(("grouping", v.as_str())),
        StandardTag::Label(v) => Some(("label", v.as_str())),
        StandardTag::SortArtist(v) => Some(("artistsort", v.as_str())),
        StandardTag::SortAlbumArtist(v) => Some(("albumartistsort", v.as_str())),
        StandardTag::SortComposer(v) => Some(("composersort", v.as_str())),
        StandardTag::Conductor(v) => Some(("conductor", v.as_str())),
        StandardTag::Mood(v) => Some(("mood", v.as_str())),
        StandardTag::Work(v) => Some(("work", v.as_str())),
        StandardTag::Ensemble(v) => Some(("ensemble", v.as_str())),
        StandardTag::MovementName(v) => Some(("movement", v.as_str())),
        StandardTag::Description(v) => Some(("comment", v.as_str())),
        StandardTag::MusicBrainzRecordingId(v) => Some(("musicbrainz_trackid", v.as_str())),
        StandardTag::MusicBrainzAlbumId(v) => Some(("musicbrainz_albumid", v.as_str())),
        StandardTag::MusicBrainzArtistId(v) => Some(("musicbrainz_artistid", v.as_str())),
        StandardTag::MusicBrainzAlbumArtistId(v) => Some(("musicbrainz_albumartistid", v.as_str())),
        StandardTag::MusicBrainzReleaseGroupId(v) => {
            Some(("musicbrainz_releasegroupid", v.as_str()))
        }
        StandardTag::MusicBrainzReleaseTrackId(v) => {
            Some(("musicbrainz_releasetrackid", v.as_str()))
        }
        StandardTag::MusicBrainzWorkId(v) => Some(("musicbrainz_workid", v.as_str())),
        _ => None,
    }
}

fn push_simple(tags: &mut Vec<(Cow<'static, str>, String)>, key: &str, value: &str) {
    if !value.is_empty() {
        tags.push((intern_tag_key(key), value.to_string()));
    }
}

/// `mixramp_start`/`mixramp_end` have no `StandardTag`. Match them case-insensitively on the
/// raw key (Vorbis comment style, or an MP4 freeform atom's `<mean>:<name>` suffix), or, for
/// ID3v2 TXXX frames, on the DESCRIPTION sub-field.
fn mixramp_key(tag: &Tag) -> Option<&'static str> {
    let candidate = tag
        .raw
        .key
        .rsplit_once(':')
        .map_or(tag.raw.key.as_str(), |(_, name)| name);
    if candidate.eq_ignore_ascii_case("mixramp_start") {
        return Some("mixramp_start");
    }
    if candidate.eq_ignore_ascii_case("mixramp_end") {
        return Some("mixramp_end");
    }
    if tag.raw.key.eq_ignore_ascii_case("TXXX") {
        let desc = tag
            .raw
            .sub_fields
            .as_ref()?
            .iter()
            .find(|f| f.field == "DESCRIPTION")?;
        if let RawValue::String(d) = &desc.value {
            if d.eq_ignore_ascii_case("mixramp_start") {
                return Some("mixramp_start");
            }
            if d.eq_ignore_ascii_case("mixramp_end") {
                return Some("mixramp_end");
            }
        }
    }
    None
}

/// Find the first ReplayGain value for one of the four fields via its `StandardTag`. Every
/// supported container (Vorbis comments, ID3v2, APE, iTunes freeform atoms) maps ReplayGain to
/// a `StandardTag`, so there is no raw-key fallback to fall back to.
fn find_replaygain(
    tags: &[Tag],
    std_match: impl Fn(&StandardTag) -> Option<&str>,
) -> Option<String> {
    tags.iter()
        .find_map(|t| t.std.as_ref().and_then(&std_match))
        .map(str::to_string)
}

fn replay_gain(tags: &[Tag]) -> (Option<f32>, Option<f32>, Option<f32>, Option<f32>) {
    let track_gain = find_replaygain(tags, |s| match s {
        StandardTag::ReplayGainTrackGain(v) => Some(v.as_str()),
        _ => None,
    })
    .and_then(|s| s.trim_end_matches(" dB").parse::<f32>().ok());

    let track_peak = find_replaygain(tags, |s| match s {
        StandardTag::ReplayGainTrackPeak(v) => Some(v.as_str()),
        _ => None,
    })
    .and_then(|s| s.parse::<f32>().ok());

    let album_gain = find_replaygain(tags, |s| match s {
        StandardTag::ReplayGainAlbumGain(v) => Some(v.as_str()),
        _ => None,
    })
    .and_then(|s| s.trim_end_matches(" dB").parse::<f32>().ok());

    let album_peak = find_replaygain(tags, |s| match s {
        StandardTag::ReplayGainAlbumPeak(v) => Some(v.as_str()),
        _ => None,
    })
    .and_then(|s| s.parse::<f32>().ok());

    (track_gain, track_peak, album_gain, album_peak)
}

/// Map a 4-byte MP4 fourcc atom identifier (as symphonia's raw key: a single Unicode
/// codepoint, e.g. `\u{a9}` COPYRIGHT SIGN, followed by 3 ASCII bytes) to a human-readable
/// key name.
fn fourcc_to_key(fourcc: &str) -> Option<&'static str> {
    match fourcc {
        "\u{a9}nam" => Some("title"),
        "\u{a9}ART" => Some("artist"),
        "\u{a9}alb" => Some("album"),
        "aART" => Some("album_artist"),
        "\u{a9}day" => Some("date"),
        "trkn" => Some("track"),
        "disk" => Some("disc"),
        "\u{a9}gen" => Some("genre"),
        "gnre" => Some("genre"),
        "\u{a9}wrt" => Some("composer"),
        "\u{a9}cmt" => Some("comment"),
        "cpil" => Some("compilation"),
        "\u{a9}grp" => Some("grouping"),
        "\u{a9}lyr" => Some("lyrics"),
        "\u{a9}too" => Some("encoder"),
        "soal" => Some("sort_album"),
        "soar" => Some("sort_artist"),
        "soaa" => Some("sort_album_artist"),
        "sonm" => Some("sort_title"),
        "soco" => Some("sort_composer"),
        "tmpo" => Some("bpm"),
        "rtng" => Some("rating"),
        "desc" => Some("description"),
        _ => None,
    }
}

/// Render a raw tag value the way MPD's `readcomments` expects: strings pass through
/// unchanged, booleans render as "1"/"0" (matching the old `AtomData::Bool` behaviour),
/// binary and flag values have no textual representation and are skipped.
fn raw_value_to_comment_string(value: &RawValue) -> Option<String> {
    match value {
        RawValue::Binary(_) | RawValue::Flag => None,
        RawValue::Boolean(b) => Some(if *b { "1" } else { "0" }.to_string()),
        _ => Some(value.to_string()),
    }
}

/// Whether two tags' `StandardTag`s are the "same shape": both absent, or both present with
/// the same variant. Symphonia splits one physical raw tag into two `Tag`s sharing an
/// identical raw key/value when it maps to two different standard tags (e.g. a Vorbis
/// "DISCNUMBER=3/12" comment becomes `DiscNumber` and `DiscTotal`); those are NOT the same
/// shape, which is exactly the signal `read_raw_comments` uses to collapse them back into one
/// raw-comment line without also collapsing genuinely repeated, identical comment lines.
fn same_std_shape(a: &Option<StandardTag>, b: &Option<StandardTag>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => std::mem::discriminant(x) == std::mem::discriminant(y),
        (None, None) => true,
        _ => false,
    }
}

#[derive(Debug, Copy, Clone)]
pub struct MetadataExtractor;

impl MetadataExtractor {
    pub fn extract_from_file(path: &Utf8PathBuf) -> Result<Song> {
        let metadata = fs::metadata(path.as_str())
            .map_err(|e| RmpdError::Library(format!("Failed to read file metadata: {e}")))?;

        let mtime = system_time_to_unix_secs(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH));

        let probed = probe_file(path, false)?;

        tracing::debug!("extracting metadata from: {}", path);
        let mut tags: Vec<(Cow<'static, str>, String)> = Vec::new();
        let mut best_original_date: Option<String> = None;

        for tag in &probed.tags {
            match &tag.std {
                Some(std) => {
                    if let Some((name, value)) = simple_std_tag(std) {
                        push_simple(&mut tags, name, value);
                    } else {
                        match std {
                            StandardTag::Comment(v) => {
                                if !v.is_empty() && !is_bogus_dsf_comment(v) {
                                    tags.push((intern_tag_key("comment"), v.to_string()));
                                }
                            }
                            StandardTag::TrackNumber(n) => {
                                if let Some(norm) = normalize_decimal(&n.to_string()) {
                                    tags.push((intern_tag_key("track"), norm));
                                }
                            }
                            StandardTag::DiscNumber(n) => {
                                if let Some(norm) = normalize_decimal(&n.to_string()) {
                                    tags.push((intern_tag_key("disc"), norm));
                                }
                            }
                            StandardTag::MovementNumber(n) => {
                                tags.push((intern_tag_key("movementnumber"), n.to_string()));
                            }
                            StandardTag::OriginalReleaseDate(v)
                                if !v.is_empty()
                                    && best_original_date
                                        .as_ref()
                                        .is_none_or(|b| v.len() > b.len()) =>
                            {
                                best_original_date = Some(v.to_string());
                            }
                            _ => {}
                        }
                    }
                }
                None => {
                    // Raw-key fallback restoring MPD tag names that symphonia's tag readers
                    // never parse into a `StandardTag` at all (plain Vorbis "COMPOSERSORT",
                    // "MOVEMENT", "MOVEMENTNUMBER", "LOCATION" comments have no registered
                    // parser upstream) -- these were only ever recognized via rmpd's own key
                    // table, not lofty's or symphonia's.
                    if let Some(name) = vorbis_tag_map_get(&tag.raw.key.to_lowercase()) {
                        let raw_val = tag.raw.value.to_string();
                        if !raw_val.is_empty() {
                            if name == "track" || name == "disc" {
                                if let Some(norm) = normalize_decimal(&raw_val) {
                                    tags.push((intern_tag_key(name), norm));
                                }
                            } else {
                                tags.push((intern_tag_key(name), raw_val));
                            }
                        }
                    }
                }
            }

            if let Some(key) = mixramp_key(tag) {
                let val = tag.raw.value.to_string();
                if !val.is_empty() {
                    tags.push((intern_tag_key(key), val));
                }
            }
        }

        // Date: prefer the recording date, falling back to a year-only value.
        let date = probed
            .tags
            .iter()
            .find_map(|t| match &t.std {
                Some(StandardTag::RecordingDate(v)) if !v.is_empty() => Some(v.to_string()),
                _ => None,
            })
            .or_else(|| {
                probed.tags.iter().find_map(|t| match &t.std {
                    Some(StandardTag::RecordingYear(y)) => Some(y.to_string()),
                    _ => None,
                })
            });
        if let Some(date) = date {
            tags.push((intern_tag_key("date"), date));
        }

        if let Some(od) = best_original_date {
            tags.push((intern_tag_key("originaldate"), od));
        }

        let (rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak) =
            replay_gain(&probed.tags);

        Ok(Song {
            id: 0,
            path: path.clone(),
            duration: probed.duration,
            sample_rate: probed.sample_rate,
            channels: probed.channels,
            bits_per_sample: {
                let ext = path.extension().map(|e| e.to_lowercase());
                match ext.as_deref() {
                    Some("m4a" | "aac") => Some(0),
                    _ => Some(probed.bit_depth.unwrap_or(16) as u16),
                }
            },
            bitrate: probed.bitrate,
            replay_gain_track_gain: rg_track_gain,
            replay_gain_track_peak: rg_track_peak,
            replay_gain_album_gain: rg_album_gain,
            replay_gain_album_peak: rg_album_peak,
            added_at: mtime,
            last_modified: mtime,
            tags,
        })
    }

    pub fn extract_artwork_from_file(path: &Utf8PathBuf) -> Result<Vec<Artwork>> {
        Ok(probe_file(path, true)?.visuals)
    }

    pub fn is_supported_file(path: &Utf8PathBuf) -> bool {
        if let Some(ext) = path.extension() {
            matches!(
                ext.to_lowercase().as_str(),
                "mp3"
                    | "flac"
                    | "ogg"
                    | "oga"
                    | "opus"
                    | "m4a"
                    | "aac"
                    | "wav"
                    | "aiff"
                    | "aif"
                    | "mka"
                    | "webm"
                    | "ape"
                    | "wv"
                    | "dsf"
                    | "dff"
            )
        } else {
            false
        }
    }

    /// Read raw key-value pairs directly from the audio file.
    ///
    /// Unlike `extract_from_file`, this returns the raw format-specific tag fields
    /// as they appear in the file, not normalized to rmpd's internal tag names.
    /// Used by the `readcomments` MPD command.
    pub fn read_raw_comments(path: &Utf8PathBuf) -> Result<Vec<(String, String)>> {
        let probed = probe_file(path, false)?;
        let ext = path
            .extension()
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        let mut pairs = Vec::new();
        // Adjacent tags with an identical raw key/value pair but a DIFFERENT `StandardTag`
        // come from symphonia splitting one physical raw tag (e.g. "3/12") into two `Tag`s,
        // one per standard tag (track number and track total): collapse those back into a
        // single raw-comment entry. Genuinely repeated, identical comment lines (same key,
        // same value, same std shape) are kept, matching MPD printing every line.
        let mut last: Option<(String, String, Option<StandardTag>)> = None;

        for tag in &probed.tags {
            let candidate: Option<(String, String)> = match ext.as_str() {
                "mp3" => {
                    if tag.raw.key != "TXXX" {
                        None
                    } else {
                        tag.raw
                            .sub_fields
                            .as_ref()
                            .and_then(|subs| subs.iter().find(|f| f.field == "DESCRIPTION"))
                            .and_then(|desc| match &desc.value {
                                RawValue::String(s) if !s.is_empty() => Some(s.to_string()),
                                _ => None,
                            })
                            .and_then(|key| {
                                raw_value_to_comment_string(&tag.raw.value).map(|v| (key, v))
                            })
                    }
                }
                "m4a" | "aac" => {
                    // trkn/disk emit a "number" tag and a "total" tag sharing one raw key with
                    // different values; only the number half should surface as a raw comment.
                    if matches!(
                        tag.std,
                        Some(StandardTag::TrackTotal(_)) | Some(StandardTag::DiscTotal(_))
                    ) {
                        None
                    } else {
                        let key = if let Some((_, name)) = tag.raw.key.rsplit_once(':') {
                            Some(name.to_string())
                        } else {
                            fourcc_to_key(&tag.raw.key).map(str::to_string)
                        };
                        key.and_then(|k| {
                            raw_value_to_comment_string(&tag.raw.value).map(|v| (k, v))
                        })
                    }
                }
                _ => raw_value_to_comment_string(&tag.raw.value).map(|v| (tag.raw.key.clone(), v)),
            };

            match candidate {
                Some(pair) => {
                    let collapse = last.as_ref().is_some_and(|(k, v, prev_std)| {
                        *k == pair.0 && *v == pair.1 && !same_std_shape(prev_std, &tag.std)
                    });
                    if !collapse {
                        pairs.push(pair.clone());
                    }
                    last = Some((pair.0, pair.1, tag.std.clone()));
                }
                None => last = None,
            }
        }

        Ok(pairs)
    }
}
