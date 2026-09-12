"""Bounded live diagnostics; never persist credentials, prompts, transcripts or audio."""
import io
import json
import math
import pathlib
import shlex
import struct
import sys
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
import wave

BASE = "http://192.168.0.130:9810"
OUT = pathlib.Path(__file__).with_name("dynamic-diagnostics.json" if "--dynamic-only" in sys.argv else "diagnostics.json")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
results = []


def request(label, path, data=None, token=None, method=None, content_type="application/json", extra=None):
    assert path.startswith("/") and not path.startswith("//")
    headers = {"Content-Type": content_type, **(extra or {})}
    if token:
        headers["Authorization"] = "Bearer " + token
    started = time.monotonic()
    record = {"case": label}
    try:
        req = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
        try:
            response = opener.open(req, timeout=30)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            body = response.read(1024 * 1024 + 1)
            record.update(status=response.status, contentType=response.headers.get("Content-Type"), bytes=len(body))
            if len(body) > 1024 * 1024:
                raise ValueError("response limit exceeded")
            try:
                value = json.loads(body)
            except (ValueError, UnicodeError):
                value = None
            if isinstance(value, dict):
                record.update({k: value[k] for k in ["releaseCommit", "configRevision", "bootEpoch"] if k in value})
                record["errorCode"] = value.get("error", {}).get("code") if isinstance(value.get("error"), dict) else None
                if "text" in value:
                    record["textPresent"] = isinstance(value["text"], str)
                    record["textNonempty"] = bool(value["text"].strip()) if isinstance(value["text"], str) else False
                if "choices" in value:
                    record["model"] = value.get("model")
                    record["finishReasons"] = [c.get("finish_reason") for c in value["choices"]]
            if "text/event-stream" in (record["contentType"] or ""):
                events = [l[5:].strip() for l in body.decode("utf-8").splitlines() if l.startswith("data:")]
                chunks = [json.loads(e) for e in events if e and e != "[DONE]"]
                record.update(done="[DONE]" in events, models=sorted({c.get("model", "") for c in chunks}),
                              finishReasons=[x.get("finish_reason") for c in chunks for x in c.get("choices", []) if x.get("finish_reason")])
            return value
    except Exception as error:
        record["transportError"] = type(error).__name__
        return None
    finally:
        record["elapsedMs"] = round((time.monotonic() - started) * 1000, 1)
        results.append(record)
        OUT.write_text(json.dumps(results, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps(record, ensure_ascii=False), flush=True)


health = request("health-before", "/health")
request("ready", "/ready")
chat = {"model": "coding-default", "messages": [{"role": "user", "content": "Reply with OK."}], "max_tokens": 64, "stream": False}
if "--dynamic-only" not in sys.argv:
    # Use the configured API credential only, through an encrypted SSH pipe and in memory.
    remote = "python3 -c " + shlex.quote("import pathlib,shlex; lines=pathlib.Path('/etc/larm/larm.env').read_text().splitlines(); print(next(shlex.split(l.split('=',1)[1])[0] for l in lines if l.startswith('LARM_API_TOKEN=')))")
    token = subprocess.check_output(["ssh", "-o", "BatchMode=yes", "gnosis", remote], timeout=10).decode().strip()
    request("authenticated-model-catalog", "/v1/models", token=token)
    request("authenticated-activity", "/v1/activity", token=token)
    request("ordinary-chat-json", "/v1/chat/completions", json.dumps(chat).encode(), token)
    request("ordinary-tts-wav", "/v1/audio/speech", json.dumps({"model": "voicevox-core", "input": "疎通確認です。", "voice": "Kasukabe_Tsumugi", "response_format": "wav"}).encode(), token)
    audio = io.BytesIO()
    with wave.open(audio, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(16000)
        wav.writeframes(b"".join(struct.pack("<h", int(2000 * math.sin(2 * math.pi * 440 * i / 16000))) for i in range(4000)))
    boundary = "saaa-" + uuid.uuid4().hex
    parts = []
    for key, value in [("model", "qwen3-asr-1.7b"), ("response_format", "json")]:
        parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{key}"\r\n\r\n{value}\r\n'.encode())
    parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="probe.wav"\r\nContent-Type: audio/wav\r\n\r\n'.encode() + audio.getvalue() + f'\r\n--{boundary}--\r\n'.encode())
    request("ordinary-asr-fixed-tone", "/v1/audio/transcriptions", b"".join(parts), token, content_type="multipart/form-data; boundary=" + boundary)
    del token
connection = request("dynamic-create", "/v1/agent-connections", json.dumps({"agentProfile": "coding-default", "audience": "saaa-desktop", "client": "saaa-http-diagnostic", "ttlSeconds": 120, "allowFallback": False, "deploymentPolicy": "existing-only"}).encode(), extra={"idempotency-key": "saaa-diagnostic-" + uuid.uuid4().hex})
if connection and isinstance(connection.get("id"), str):
    path = "/v1/agent-connections/" + urllib.parse.quote(connection["id"], safe="")
    try:
        for _ in range(10):
            if connection.get("status") == "ready":
                break
            time.sleep(0.5)
            connection = request("dynamic-poll", path) or {}
        claim = request("dynamic-claim", path + "/claim", b'{"format":"openai-provider-v1"}')
        if claim and claim.get("providers"):
            provider = claim["providers"][0]
            parsed = urllib.parse.urlsplit(provider["baseUrl"])
            assert (parsed.scheme, parsed.netloc) == ("http", "192.168.0.130:9810")
            key = provider["credential"]["token"]
            chat["model"] = provider["model"]
            chat["stream"] = True
            request("claimed-chat-sse", parsed.path.rstrip("/") + "/chat/completions", json.dumps(chat).encode(), key)
            released = request("dynamic-release", path, method="DELETE")
            request("released-credential-health", urllib.parse.urlsplit(provider["health"]["url"]).path, token=key)
            del key
    finally:
        if not any(r["case"] == "dynamic-release" and r.get("status") in (200, 204) for r in results):
            request("dynamic-release-finally", path, method="DELETE")
        final_state = request("dynamic-state-after-release", path)
        if final_state:
            results.append({"case": "cleanup-state", "status": final_state.get("status")})
request("health-after", "/health")
