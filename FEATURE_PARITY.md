# rmpd vs MPD — Feature Parity Report

**Date**: March 2026  
**MPD source**: https://github.com/MusicPlayerDaemon/MPD (HEAD)  
**rmpd source**: https://github.com/M0Rf30/rmpd v0.2.0

Legend: ✅ implemented · 🚧 partial/planned · ❌ missing

---

## 1. Protocol Commands (MPD has 95)

### Playback Control
| Command | MPD | rmpd |
|---------|-----|------|
| `play [pos]` | ✅ | ✅ |
| `playid <id>` | ✅ | ✅ |
| `pause [0\|1]` | ✅ | ✅ |
| `stop` | ✅ | ✅ |
| `next` | ✅ | ✅ |
| `previous` | ✅ | ✅ |
| `seek <pos> <time>` | ✅ | ✅ |
| `seekid <id> <time>` | ✅ | ✅ |
| `seekcur <offset>` | ✅ | ✅ |

### Queue / Playlist Management
| Command | MPD | rmpd |
|---------|-----|------|
| `add <uri>` | ✅ | ✅ |
| `addid <uri> [pos]` | ✅ | ✅ |
| `delete <pos>` | ✅ | ✅ |
| `deleteid <id>` | ✅ | ✅ |
| `clear` | ✅ | ✅ |
| `move <from> <to>` | ✅ | ✅ |
| `moveid <id> <pos>` | ✅ | ✅ |
| `swap <pos1> <pos2>` | ✅ | ✅ |
| `swapid <id1> <id2>` | ✅ | ✅ |
| `shuffle [range]` | ✅ | ✅ |
| `prio <prio> <pos>...` | ✅ | ✅ |
| `prioid <prio> <id>...` | ✅ | ✅ |
| `rangeid <id> <s>:<e>` | ✅ | ✅ |
| `playlist` | ✅ | ✅ |
| `playlistinfo [range]` | ✅ | ✅ |
| `playlistid [id]` | ✅ | ✅ |
| `playlistlength` | ✅ | ✅ |
| `plchanges <ver>` | ✅ | ✅ |
| `plchangesposid <ver>` | ✅ | ✅ |

### Saved Playlist Commands
| Command | MPD | rmpd |
|---------|-----|------|
| `save <name>` | ✅ | ✅ |
| `load <name>` | ✅ | ✅ |
| `rm <name>` | ✅ | ✅ |
| `rename <old> <new>` | ✅ | ✅ |
| `listplaylists` | ✅ | ✅ |
| `listplaylist <name>` | ✅ | ✅ |
| `listplaylistinfo <name>` | ✅ | ✅ |
| `playlistadd <name> <uri>` | ✅ | ✅ |
| `playlistdelete <name> <pos>` | ✅ | ✅ |
| `playlistclear <name>` | ✅ | ✅ |
| `playlistmove <name> <f> <t>` | ✅ | ✅ |
| `playlistfind <name> ...` | ✅ | ✅ |
| `playlistsearch <name> ...` | ✅ | ✅ |

### Status & Information
| Command | MPD | rmpd |
|---------|-----|------|
| `status` | ✅ | ✅ |
| `currentsong` | ✅ | ✅ |
| `stats` | ✅ | ✅ |
| `getvol` | ✅ | ✅ |
| `setvol <vol>` | ✅ | ✅ |
| `volume <delta>` | ✅ | ✅ |
| `replay_gain_status` | ✅ | ✅ |
| `replay_gain_mode <mode>` | ✅ | ✅ |

### Playback Options
| Command | MPD | rmpd |
|---------|-----|------|
| `repeat [0\|1]` | ✅ | ✅ |
| `random [0\|1]` | ✅ | ✅ |
| `single [0\|1\|oneshot]` | ✅ | ✅ |
| `consume [0\|1\|oneshot]` | ✅ | ✅ |
| `crossfade <secs>` | ✅ | ✅ |
| `mixrampdb <db>` | ✅ | ✅ |
| `mixrampdelay <secs>` | ✅ | ✅ |

### Database / Search
| Command | MPD | rmpd |
|---------|-----|------|
| `find <filter>` | ✅ | ✅ |
| `findadd <filter>` | ✅ | ✅ |
| `search <filter>` | ✅ | ✅ |
| `searchadd <filter>` | ✅ | ✅ |
| `searchaddpl <pl> <filter>` | ✅ | ✅ |
| `searchcount <filter>` | ✅ | ❌ |
| `count <filter>` | ✅ | ✅ |
| `list <tag> [filter]` | ✅ | ✅ |
| `listall [dir]` | ✅ | ✅ |
| `listallinfo [dir]` | ✅ | ✅ |
| `lsinfo [dir]` | ✅ | ✅ |
| `listfiles [dir]` | ✅ | ✅ |
| `update [path]` | ✅ | ✅ |
| `rescan [path]` | ✅ | ✅ |

### Output Commands
| Command | MPD | rmpd |
|---------|-----|------|
| `outputs` | ✅ | ✅ |
| `enableoutput <id>` | ✅ | ✅ |
| `disableoutput <id>` | ✅ | ✅ |
| `toggleoutput <id>` | ✅ | ✅ |
| `outputset <id> <k> <v>` | ✅ | ✅ |
| `moveoutput <id> <part>` | ✅ | ✅ |

### Partition Commands
| Command | MPD | rmpd |
|---------|-----|------|
| `partition <name>` | ✅ | ✅ |
| `listpartitions` | ✅ | ✅ |
| `newpartition <name>` | ✅ | ✅ |
| `delpartition <name>` | ✅ | ✅ |

### Sticker Commands
| Command | MPD | rmpd |
|---------|-----|------|
| `sticker get/set/delete/list/find` | ✅ | ✅ |
| `stickernames` | ✅ | ❌ |
| `stickernamestypes` | ✅ | ❌ |
| `stickertypes` | ✅ | ❌ |

### Messaging
| Command | MPD | rmpd |
|---------|-----|------|
| `subscribe <ch>` | ✅ | ✅ |
| `unsubscribe <ch>` | ✅ | ✅ |
| `channels` | ✅ | ✅ |
| `sendmessage <ch> <msg>` | ✅ | ✅ |
| `readmessages` | ✅ | ✅ |

### Tag / Metadata Commands
| Command | MPD | rmpd |
|---------|-----|------|
| `addtagid <id> <tag> <val>` | ✅ | ❌ |
| `cleartagid <id> [tag]` | ✅ | ❌ |
| `tagtypes` | ✅ | ✅ |
| `readcomments <uri>` | ✅ | ✅ |
| `readpicture <uri>` | ✅ | ✅ |
| `albumart <uri>` | ✅ | ✅ |

### Storage / Mount
| Command | MPD | rmpd |
|---------|-----|------|
| `mount <path> <uri>` | ✅ | ✅ |
| `unmount <path>` | ✅ | ✅ |
| `listmounts` | ✅ | ✅ |
| `listneighbors` | ✅ | ✅ |

### Fingerprint
| Command | MPD | rmpd |
|---------|-----|------|
| `getfingerprint <uri>` | ✅ | ✅ |

### Connection / Server
| Command | MPD | rmpd |
|---------|-----|------|
| `ping` | ✅ | ✅ |
| `close` | ✅ | ✅ |
| `kill` | ✅ | ✅ |
| `password <pw>` | ✅ | 🚧 placeholder |
| `commands` | ✅ | ✅ |
| `notcommands` | ✅ | ✅ |
| `config` | ✅ | ✅ |
| `protocol` | ✅ | ✅ |
| `idle [subsystems]` | ✅ | ✅ |
| `noidle` | ✅ | ✅ |
| `binarylimit` | ✅ | ✅ |
| `stringnormalization` | ✅ | ❌ |
| `urlhandlers` | ✅ | ✅ |
| `decoders` | ✅ | ✅ |

### Command Batching
| Feature | MPD | rmpd |
|---------|-----|------|
| `command_list_begin` | ✅ | ✅ |
| `command_list_ok_begin` | ✅ | ✅ |
| `command_list_end` | ✅ | ✅ |

### Command Parity Summary
| Status | Count |
|--------|-------|
| ✅ Implemented | ~88 |
| ❌ Missing | 7 (`searchcount`, `stickernames`, `stickernamestypes`, `stickertypes`, `addtagid`, `cleartagid`, `stringnormalization`) |

---

## 2. Audio Decoders / Formats

MPD supports 37 decoder plugins. rmpd uses Symphonia (+ custom DSD fork).

| Format / Plugin | MPD | rmpd |
|-----------------|-----|------|
| FLAC | ✅ | ✅ |
| WAV / AIFF | ✅ | ✅ |
| MP3 (mad / mpg123) | ✅ | ✅ (Symphonia) |
| AAC / M4A | ✅ | ✅ |
| Ogg Vorbis | ✅ | ✅ |
| Opus | ✅ | ✅ |
| ALAC | ✅ | ✅ |
| WavPack | ✅ | ✅ |
| APE (Monkey's Audio) | ✅ | ✅ |
| TrueAudio (TTA) | ✅ | ✅ |
| DSD (DSF, DSDIFF/DFF) | ✅ | ✅ |
| DoP (DSD over PCM) | ✅ | ✅ |
| Musepack (mpc) | ✅ | ❌ |
| Raw PCM (audio/L16) | ✅ | ❌ |
| Sndfile formats | ✅ | ❌ |
| MOD / XM / IT / S3M (tracker) | ✅ (MikMod/Modplug/OpenMPT) | ❌ |
| SID (C64) | ✅ | ❌ |
| GME (chiptune/game music) | ✅ | ❌ |
| VGMStream | ✅ | ❌ |
| MIDI (FluidSynth / WildMidi) | ✅ | ❌ |
| AdPlug (AdLib OPL) | ✅ | ❌ |

**Summary**: rmpd covers all mainstream formats through Symphonia. Niche formats (tracker, chiptune, MIDI, game audio, AdLib) are not supported.

---

## 3. Audio Outputs

| Output Plugin | MPD | rmpd |
|---------------|-----|------|
| ALSA | ✅ | ✅ |
| PulseAudio | ✅ | ✅ |
| PipeWire | ✅ | ✅ |
| Snapcast | ✅ | 🚧 in progress |
| JACK | ✅ | ❌ |
| OSS | ✅ | ❌ |
| Sndio (OpenBSD/NetBSD) | ✅ | ❌ |
| libao | ✅ | ❌ |
| OpenAL | ✅ | ❌ |
| HTTP Streaming (httpd) | ✅ | ❌ |
| Shoutcast / Icecast (libshout) | ✅ | ❌ |
| FIFO | ✅ | ❌ |
| Pipe (external program) | ✅ | ❌ |
| Recorder (to file) | ✅ | ❌ |
| macOS (CoreAudio) | ✅ | ❌ |
| Windows (WASAPI) | ✅ | ❌ |

**Summary**: rmpd covers the three dominant Linux audio systems. Missing: JACK (pro audio), HTTP/Shoutcast (streaming server), FIFO/Pipe/Recorder (file/program outputs), cross-platform (macOS, Windows, OSS, Sndio, libao).

---

## 4. Input Protocols

| Input Plugin | MPD | rmpd |
|--------------|-----|------|
| Local filesystem | ✅ | ✅ |
| HTTP / HTTPS | ✅ | ✅ |
| NFS | ✅ | ✅ |
| SMB / CIFS | ✅ | ✅ |
| WebDAV | ✅ | ✅ |
| FTP / FTPS | ✅ | ✅ |
| SFTP / SCP | ✅ | ✅ |
| RTSP / RTMP | ✅ | ❌ |
| MMS (Microsoft) | ✅ | ❌ |
| RTP | ✅ | ❌ |
| Gopher | ✅ | ❌ |
| ALSA audio input (`alsa://`) | ✅ | ❌ |
| CD Audio (`cdda://`) | ✅ | ❌ |
| Qobuz streaming service | ✅ | ❌ |
| Archive (ZIP, ISO, bzip2) | ✅ | ❌ |

---

## 5. Tags

Both support all 47 MPD tag types including MusicBrainz IDs, classical music tags, and sort variants. ✅ Full parity.

---

## 6. Playlist Formats

| Format | MPD | rmpd |
|--------|-----|------|
| M3U | ✅ | ✅ |
| PLS | ✅ | ❌ |
| XSPF | ✅ | ❌ |
| CUE sheet | ✅ | ❌ |
| Embedded CUE | ✅ | ❌ |
| ASX | ✅ | ❌ |

---

## 7. Database Plugins

| Plugin | MPD | rmpd |
|--------|-----|------|
| Simple (file-backed) | ✅ | ✅ (SQLite) |
| Proxy (remote MPD via libmpdclient) | ✅ | ❌ |
| UPnP media server | ✅ | ❌ |

---

## 8. Encoder Plugins (for streaming output)

| Encoder | MPD | rmpd |
|---------|-----|------|
| Ogg Vorbis | ✅ | ❌ |
| LAME MP3 | ✅ | ❌ |
| TwoLAME MP2 | ✅ | ❌ |
| Shine MP3 | ✅ | ❌ |
| WAV PCM | ✅ | ❌ |

*Encoders are needed for HTTP streaming / Shoutcast output; not relevant without those outputs.*

---

## 9. Archive Support

| Format | MPD | rmpd |
|--------|-----|------|
| ZIP | ✅ | ❌ |
| ISO 9660 | ✅ | ❌ |
| Bzip2 | ✅ | ❌ |

---

## 10. Audio Processing

| Feature | MPD | rmpd |
|---------|-----|------|
| Gapless playback | ✅ | ✅ |
| ReplayGain (off/track/album/auto) | ✅ | ✅ |
| Crossfade | ✅ | ✅ |
| MixRamp | ✅ | 🚧 |
| Resampling (libsamplerate / libsoxr) | ✅ | ✅ (rubato) |
| DSD native playback | ✅ | ✅ |
| DoP (DSD over PCM) | ✅ | ✅ |
| Volume normalization | ✅ | ✅ |

---

## 11. System Integration

| Feature | MPD | rmpd |
|---------|-----|------|
| TCP socket | ✅ | ✅ |
| Unix domain socket | ✅ | ✅ |
| IPv6 | ✅ | 🚧 |
| Authentication / permissions | ✅ | 🚧 (placeholder) |
| Fine-grained permission system | ✅ | ❌ |
| Systemd service | ✅ | ✅ |
| Daemon mode / daemonization | ✅ | ❌ |
| Syslog | ✅ | ❌ |
| Zeroconf / mDNS (Avahi/Bonjour) | ✅ | ❌ |
| D-Bus integration | ✅ | ❌ |
| io_uring async I/O | ✅ | ❌ |
| inotify (auto library updates) | ✅ | ✅ |

---

## 12. Neighbor Discovery

| Plugin | MPD | rmpd |
|--------|-----|------|
| SMB discovery | ✅ | ❌ |
| UDisks (removable media) | ✅ | ❌ |
| UPnP discovery | ✅ | ❌ |

---

## 13. Protocol & Connection Features

| Feature | MPD | rmpd |
|---------|-----|------|
| Command pipelining | ✅ | ✅ |
| Command batching (command_list) | ✅ | ✅ |
| Binary data responses | ✅ | ✅ |
| Filter expressions | ✅ | ✅ |
| Range syntax | ✅ | ✅ |
| Idle / event notifications | ✅ | ✅ |
| All idle subsystems | ✅ | ✅ |
| Password auth | ✅ | 🚧 |
| Permission levels (read/add/control/player/admin) | ✅ | ❌ |

---

## Gap Summary

### High Priority (core MPD functionality)
| Gap | Category |
|-----|----------|
| Permission system (read/add/control/player/admin) | Security |
| `password` command (real auth, not placeholder) | Security |
| `addtagid` / `cleartagid` commands | Protocol |
| `searchcount` command | Protocol |
| `stickernames` / `stickernamestypes` / `stickertypes` | Protocol |
| `stringnormalization` command | Protocol |
| Daemonization (`--daemonize`) | System |
| PLS / XSPF / CUE playlist formats | Library |
| FIFO output | Output |
| Pipe output (to external program) | Output |
| Recorder output (record to file) | Output |

### Medium Priority (common use-cases)
| Gap | Category |
|-----|----------|
| JACK output (pro audio / studio) | Output |
| HTTP streaming output (httpd) | Output |
| Shoutcast / Icecast output | Output |
| macOS CoreAudio output | Output |
| Proxy database plugin (remote MPD) | Database |
| Musepack decoder | Decoder |
| Archive input (ZIP / ISO) | Input |
| CD Audio input (`cdda://`) | Input |
| Zeroconf / mDNS service discovery | System |
| IPv6 support | Network |

### Low Priority (niche / platform-specific)
| Gap | Category |
|-----|----------|
| Tracker formats (MOD/XM/IT/S3M) | Decoder |
| Chiptune / game music (GME, SID, VGMStream) | Decoder |
| MIDI (FluidSynth / WildMidi) | Decoder |
| AdPlug (AdLib) | Decoder |
| OSS / Sndio / libao / OpenAL output | Output |
| Windows WASAPI output | Output |
| Solaris /dev/audio output | Output |
| MMS / RTP / RTSP / RTMP input | Input |
| ALSA audio input (`alsa://`) | Input |
| Qobuz streaming service | Input |
| UPnP database / neighbor discovery | Database |
| D-Bus integration | System |
| Syslog | System |
| io_uring async I/O | System |
| Encoder plugins (Vorbis, LAME, etc.) | Encoder |

---

## Overall Parity Score

| Category | MPD Total | rmpd | Coverage |
|----------|-----------|------|----------|
| Commands | 95 | ~88 | **93%** |
| Decoders | 37 | ~10 | **27%** (100% of mainstream formats) |
| Outputs | 16 | 4 | **25%** (covers 3 dominant Linux outputs) |
| Input protocols | 11 | 7 | **64%** |
| Tag types | 47 | 47 | **100%** |
| Playlist formats | 6 | 1 | **17%** |
| Database plugins | 3 | 1 | **33%** |
| System integration | — | — | **~60%** |

**rmpd covers ~93% of commands and 100% of mainstream audio formats/tags, making it a solid drop-in replacement for the most common MPD use-cases. The primary remaining gaps are: permission/auth system, additional output backends (JACK, HTTP streaming, FIFO, Recorder, macOS), CUE/XSPF/PLS playlist formats, and niche decoders (tracker, chiptune, MIDI).**
