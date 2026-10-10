#!/bin/sh
# Renders every example listed in examples.txt to docs/public/audio/*.mp3.
# Each line: <file.nml> [arrangement]. Needs `niminal` on PATH and ffmpeg.
set -e
cd "$(dirname "$0")/../.."
mkdir -p docs/public/audio
while read -r file arrangement; do
  [ -z "$file" ] && continue
  name="${file%.nml}"
  niminal render "examples/$file" $arrangement --out "docs/public/audio/$name.wav"
  ffmpeg -y -loglevel error -i "docs/public/audio/$name.wav" -b:a 128k "docs/public/audio/$name.mp3"
  rm "docs/public/audio/$name.wav"
done < docs/scripts/examples.txt
