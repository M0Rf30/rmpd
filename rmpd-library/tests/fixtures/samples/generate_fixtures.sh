#!/bin/bash
# Generate test audio fixtures for rmpd compatibility tests
# These files are committed to the repo to avoid FFmpeg dependency

set -e

# Check FFmpeg is available
if ! command -v ffmpeg &> /dev/null; then
    echo "FFmpeg is required to generate fixtures"
    exit 1
fi

echo "Generating test audio fixtures..."

# Basic FLAC file
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -ar 44100 -ac 2 \
    -metadata title="Test Song" \
    -metadata artist="Test Artist" \
    -metadata album="Test Album" \
    -metadata genre="Rock" \
    -metadata date="2024" \
    -metadata track="1" \
    -y basic.flac 2>/dev/null

# Basic MP3 file
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -ar 44100 -ac 2 \
    -codec:a libmp3lame -q:a 2 \
    -metadata title="Test Song MP3" \
    -metadata artist="Test Artist MP3" \
    -metadata album="Test Album MP3" \
    -metadata genre="Pop" \
    -y basic.mp3 2>/dev/null

# Basic OGG file
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -ar 44100 -ac 2 \
    -codec:a libvorbis -b:a 128k \
    -metadata title="Test Song OGG" \
    -metadata artist="Test Artist OGG" \
    -metadata album="Test Album OGG" \
    -y basic.ogg 2>/dev/null

# Basic Opus file (requires 48kHz)
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -ar 48000 -ac 2 \
    -codec:a libopus -b:a 128k \
    -metadata title="Test Song Opus" \
    -metadata artist="Test Artist Opus" \
    -metadata album="Test Album Opus" \
    -y basic.opus 2>/dev/null

# Basic M4A file
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -ar 44100 -ac 2 \
    -codec:a aac -b:a 192k \
    -metadata title="Test Song M4A" \
    -metadata artist="Test Artist M4A" \
    -metadata album="Test Album M4A" \
    -y basic.m4a 2>/dev/null

# Basic WAV file
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -ar 44100 -ac 2 \
    -codec:a pcm_s16le \
    -metadata title="Test Song WAV" \
    -metadata artist="Test Artist WAV" \
    -metadata album="Test Album WAV" \
    -y basic.wav 2>/dev/null

# Unicode metadata FLAC
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -ar 44100 -ac 2 \
    -metadata title="テストソング" \
    -metadata artist="Тестовый исполнитель" \
    -metadata album="Τεστ Άλμπουμ" \
    -metadata genre="الموسيقى" \
    -y unicode.flac 2>/dev/null

# Minimal metadata (no optional fields)
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -ar 44100 -ac 2 \
    -metadata title="Minimal" \
    -metadata artist="Artist" \
    -metadata album="Album" \
    -y minimal.flac 2>/dev/null

# File with extended metadata
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -ar 44100 -ac 2 \
    -metadata title="Extended Metadata" \
    -metadata artist="Extended Artist" \
    -metadata album="Extended Album" \
    -metadata album_artist="Various Artists" \
    -metadata composer="Test Composer" \
    -metadata genre="Jazz" \
    -metadata date="2024-03-15" \
    -metadata track="5" \
    -metadata disc="2" \
    -y extended.flac 2>/dev/null

echo "Generated fixtures:"
ls -lh *.{flac,mp3,ogg,opus,m4a,wav} 2>/dev/null | awk '{print $9, $5}'
echo ""
echo "Total size:"
du -sh . | awk '{print $1}'
