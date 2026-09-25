import os
from pathlib import Path
source = Path(__file__).with_name("diagnose.py").read_text()
exec(source.split('health = request("health-before"')[0])
OUT = Path(__file__).with_name("speech-reproduction.json")
request("health-before", "/health")
remote = "python3 -c " + shlex.quote("import pathlib,shlex; lines=pathlib.Path('/etc/larm/larm.env').read_text().splitlines(); print(next(shlex.split(l.split('=',1)[1])[0] for l in lines if l.startswith('LARM_API_TOKEN=')))")
token = subprocess.check_output(["ssh", "-o", "BatchMode=yes", "-i", os.path.expanduser("~/.ssh/ai395.pem"), "ugnoguchi@192.168.0.130", remote], timeout=10).decode().strip()
def transcribe(label, audio):
    boundary = "saaa-" + uuid.uuid4().hex
    parts=[]
    for key,value in [("model","qwen3-asr-1.7b"),("response_format","json")]:
        parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{key}"\r\n\r\n{value}\r\n'.encode())
    parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="fixed-audio.wav"\r\nContent-Type: audio/wav\r\n\r\n'.encode()+audio+f'\r\n--{boundary}--\r\n'.encode())
    value=request(label,"/v1/audio/transcriptions",b"".join(parts),token,content_type="multipart/form-data; boundary="+boundary)
    if isinstance(value,dict) and isinstance(value.get("text"),str):
        print("synthetic_audio_recognition:", repr(value["text"][:120]))
        results[-1]["normalizedMatch"] = "".join(c for c in value["text"] if not c.isspace() and c not in "、。,.！!？?") == "疎通確認です"
        results[-1]["expectedContentMatched"] = ("疎通確認" in value["text"]) if label=="asr-japanese-speech" else value["text"].strip()==""
        OUT.write_text(json.dumps(results,ensure_ascii=False,indent=2)+"\n")
for kind in []:
    audio=io.BytesIO()
    with wave.open(audio,"wb") as wav:
        wav.setnchannels(1);wav.setsampwidth(2);wav.setframerate(16000)
        wav.writeframes(b"".join(struct.pack("<h",int(2000*math.sin(2*math.pi*440*i/16000)) if kind=="tone" else 0) for i in range(4000)))
    transcribe("asr-"+kind,audio.getvalue())
audio=request("synthetic-japanese-source","/v1/audio/speech",json.dumps({"model":"voicevox-core","input":"疎通確認です。","voice":"Kasukabe_Tsumugi","response_format":"wav"}).encode(),token)
if isinstance(audio,bytes):transcribe("asr-japanese-speech",audio)
del token
request("health-after","/health")
