#!/bin/bash
# Generate pre-generated test fixtures for decoder tests
# Requires: FFmpeg

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "Generating decoder test fixtures..."

# 1kHz sine wave fixtures (primary test files)
# Note: FFmpeg's sine generator produces low amplitude by default (~0.088)
# We apply volume filter to increase to ~0.8 for better test signal
echo "  - sine_1khz.flac (1kHz, 44.1kHz, stereo, 1s)"
ffmpeg -y -f lavfi -i "sine=frequency=1000:duration=1" -af "volume=9" -ar 44100 -ac 2 sine_1khz.flac 2>/dev/null

echo "  - sine_1khz.mp3 (1kHz, 44.1kHz, stereo, 1s, VBR)"
ffmpeg -y -f lavfi -i "sine=frequency=1000:duration=1" -af "volume=9" -ar 44100 -ac 2 -codec:a libmp3lame -q:a 2 sine_1khz.mp3 2>/dev/null

echo "  - sine_1khz.ogg (1kHz, 44.1kHz, stereo, 1s)"
ffmpeg -y -f lavfi -i "sine=frequency=1000:duration=1" -af "volume=9" -ar 44100 -ac 2 -codec:a libvorbis -q:a 5 sine_1khz.ogg 2>/dev/null

echo "  - sine_1khz.opus (1kHz, 48kHz, stereo, 1s)"
ffmpeg -y -f lavfi -i "sine=frequency=1000:duration=1" -af "volume=9" -ar 48000 -ac 2 -codec:a libopus -b:a 128k sine_1khz.opus 2>/dev/null

echo "  - sine_1khz.m4a (1kHz, 44.1kHz, stereo, 1s)"
ffmpeg -y -f lavfi -i "sine=frequency=1000:duration=1" -af "volume=9" -ar 44100 -ac 2 -codec:a aac -b:a 192k sine_1khz.m4a 2>/dev/null

echo "  - sine_1khz.wav (1kHz, 44.1kHz, stereo, 1s, 16-bit PCM)"
ffmpeg -y -f lavfi -i "sine=frequency=1000:duration=1" -af "volume=9" -ar 44100 -ac 2 -codec:a pcm_s16le sine_1khz.wav 2>/dev/null

# 440Hz for seek tests
echo "  - sine_440hz.flac (440Hz, 44.1kHz, stereo, 1s)"
ffmpeg -y -f lavfi -i "sine=frequency=440:duration=1" -af "volume=9" -ar 44100 -ac 2 sine_440hz.flac 2>/dev/null

# Silence for artifact detection
echo "  - silence.flac (silence, 44.1kHz, stereo, 0.5s)"
ffmpeg -y -f lavfi -i "anullsrc=duration=0.5" -ar 44100 -ac 2 silence.flac 2>/dev/null

# High-resolution audio (optional)
echo "  - highres.flac (1kHz, 96kHz, stereo, 1s, 24-bit)"
ffmpeg -y -f lavfi -i "sine=frequency=1000:duration=1" -af "volume=9" -ar 96000 -ac 2 -sample_fmt s32 highres.flac 2>/dev/null

# Mono file (optional)
echo "  - mono.flac (440Hz, 44.1kHz, mono, 1s)"
ffmpeg -y -f lavfi -i "sine=frequency=440:duration=1" -af "volume=9" -ar 44100 -ac 1 mono.flac 2>/dev/null

echo ""
echo "Generated fixtures:"
ls -lh *.flac *.mp3 *.ogg *.opus *.m4a *.wav 2>/dev/null | awk '{print "  " $9 " (" $5 ")"}'

echo ""
TOTAL_SIZE=$(du -sh . | awk '{print $1}')
echo "Total size: $TOTAL_SIZE"
echo ""
echo "Fixtures generated successfully!"
