import os
from pathlib import Path
source = Path(__file__).with_name("diagnose.py").read_text()
exec(source.split('health = request("health-before"')[0])
OUT = Path(__file__).with_name("followup.json")
remote = "python3 -c " + shlex.quote("import pathlib,shlex; lines=pathlib.Path('/etc/larm/larm.env').read_text().splitlines(); print(next(shlex.split(l.split('=',1)[1])[0] for l in lines if l.startswith('LARM_API_TOKEN=')))")
token = subprocess.check_output(["ssh", "-o", "BatchMode=yes", "-i", os.path.expanduser("~/.ssh/ai395.pem"), "ugnoguchi@192.168.0.130", remote], timeout=10).decode().strip()
request("health-before-followup", "/health")
chat = {"model":"coding-default","messages":[{"role":"user","content":"Reply with OK."}],"max_tokens":64,"stream":False}
request("json-reproduction-once", "/v1/chat/completions", json.dumps(chat).encode(), token)
chat.update(stream=True, reasoning_effort="low", max_tokens=512)
request("sse-with-saaa-reasoning-effort", "/v1/chat/completions", json.dumps(chat).encode(), token)
audio = request("tts-wav-format", "/v1/audio/speech", json.dumps({"model":"voicevox-core","input":"疎通確認です。","voice":"Kasukabe_Tsumugi","response_format":"wav"}).encode(), token)
if isinstance(audio, bytes):
    boundary = "saaa-" + uuid.uuid4().hex
    parts = []
    for key,value in [("model","qwen3-asr-1.7b"),("response_format","json")]:
        parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{key}"\r\n\r\n{value}\r\n'.encode())
    parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="synthetic-speech.wav"\r\nContent-Type: audio/wav\r\n\r\n'.encode()+audio+f'\r\n--{boundary}--\r\n'.encode())
    request("asr-synthetic-japanese-speech", "/v1/audio/transcriptions", b"".join(parts), token, content_type="multipart/form-data; boundary="+boundary)
del token
request("health-after-followup", "/health")
