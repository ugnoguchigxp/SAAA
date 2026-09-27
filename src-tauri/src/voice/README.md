# Voice

The legacy ASR session owner and streaming TTS scheduler were removed for a clean rebuild. Do not reconnect them to the ordinary conversation turn path.

Low-level audio I/O, speaker enrollment, network protocol code, and provider settings remain as independent resources. The replacement Listening and Speaking owners are described in the [response runtime rebuild proposal](../../../spec/docs/saaa-jarvis-response-runtime-rebuild-proposal.md).
