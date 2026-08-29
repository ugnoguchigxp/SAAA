#!/bin/sh
set -eu

root_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
voice_dir="$root_dir/src-tauri/resources/voice"
temp_dir=$(mktemp -d)
archive="$temp_dir/sherpa-onnx.tar.bz2"
model="$temp_dir/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx"

archive_url="https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.6/sherpa-onnx-v1.13.6-osx-universal2-shared-no-tts-lib.tar.bz2"
archive_sha="812b144d199fd9a5b8ccbe4a5d81df8b8f55fc28212523b9dee9cacbc9fc5a76"
model_url="https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx"
model_sha="f682b514c05d947ee3fa91cd6ec6c5c7543479a128373fa29b1faedccd21fd11"

curl -fL --retry 3 "$archive_url" -o "$archive"
curl -fL --retry 3 "$model_url" -o "$model"

verify_sha() {
  expected=$1
  path=$2
  actual=$(shasum -a 256 "$path" | awk '{print $1}')
  if [ "$actual" != "$expected" ]; then
    echo "SHA-256 mismatch for $path" >&2
    exit 1
  fi
}

verify_sha "$archive_sha" "$archive"
verify_sha "$model_sha" "$model"

mkdir -p "$voice_dir/lib" "$voice_dir/model"
tar -xjf "$archive" -C "$temp_dir"
distribution="$temp_dir/sherpa-onnx-v1.13.6-osx-universal2-shared-no-tts-lib"
cp "$distribution/lib/libsherpa-onnx-c-api.dylib" "$voice_dir/lib/"
cp "$distribution/lib/libonnxruntime.dylib" "$voice_dir/lib/"
cp "$model" "$voice_dir/model/"

verify_sha "$model_sha" "$voice_dir/model/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx"
echo "Installed the pinned local speaker-verification runtime in $voice_dir"
