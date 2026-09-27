import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import "./ttsDictionaryPage.css";

type Entry = { written: string; spoken: string };
type PresetSearchResult = { total: number; entries: Entry[] };
type ExistingEntry = { source: "custom" | "preset"; entry: Entry };

export function TtsDictionaryPage() {
  const [entries, setEntries] = useState<Entry[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [selectedSource, setSelectedSource] = useState<"custom" | "preset" | null>(null);
  const [presets, setPresets] = useState<PresetSearchResult | null>(null);
  const [checkedWritten, setCheckedWritten] = useState<{ written: string; existing: ExistingEntry | null } | null>(null);
  const [written, setWritten] = useState("");
  const [spoken, setSpoken] = useState("");
  const [skip, setSkip] = useState(false);
  const [search, setSearch] = useState("");
  const [sample, setSample] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [played, setPlayed] = useState(false);

  useEffect(() => {
    let active = true;
    setPresets(null);
    void invoke<Entry[]>("list_tts_dictionary")
      .then((items) => active && setEntries(items))
      .catch((cause) => active && setError(String(cause)));
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    const timer = window.setTimeout(() => {
      void invoke<PresetSearchResult>("search_tts_dictionary_presets", { query: search.trim() })
        .then((result) => active && setPresets(result))
        .catch((cause) => active && setError(String(cause)));
    }, search.trim() ? 150 : 0);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [search, entries]);

  useEffect(() => {
    const candidate = written.trim();
    if (!candidate || (selected === candidate && selectedSource !== null) || entries.some((entry) => entry.written === candidate)) return;
    let active = true;
    void invoke<ExistingEntry | null>("lookup_tts_dictionary_entry", { written: candidate })
      .then((existing) => active && setCheckedWritten({ written: candidate, existing }))
      .catch((cause) => active && setError(String(cause)));
    return () => { active = false; };
  }, [written, selected, selectedSource, entries]);

  function select(entry: Entry | null, source: "custom" | "preset" | null = null) {
    setSelected(entry?.written ?? null);
    setSelectedSource(source);
    setWritten(entry?.written ?? "");
    setSpoken(entry?.spoken ?? "");
    setSkip(entry?.spoken === "");
    setSample(entry ? `${entry.written}に相談してみましょう。` : "");
    setError("");
    setPlayed(false);
  }

  async function save() {
    if (duplicate || checkingDuplicate) return;
    if (!written.trim() || (!skip && !spoken.trim())) {
      setError("表記と読み上げ方を入力してください。");
      return;
    }
    setBusy(true);
    setError("");
    try {
      const entry = { written: written.trim(), spoken: skip ? "" : spoken.trim() };
      const saved = await invoke<Entry[]>("save_tts_dictionary_entry", {
        input: {
          original: selectedSource === "custom" ? selected : null,
          presetOverride: selectedSource === "preset" ? selected : null,
          entry,
        },
      });
      setEntries(saved);
      setSelected(entry.written);
      setSelectedSource("custom");
      setWritten(entry.written);
      setSpoken(entry.spoken);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function remove() {
    if (!selected || selectedSource !== "custom") return;
    setBusy(true);
    setError("");
    try {
      setEntries(await invoke<Entry[]>("delete_tts_dictionary_entry", { written: selected }));
      select(null);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function preview() {
    if (!sample.trim()) return;
    setBusy(true);
    setError("");
    setPlayed(false);
    try {
      await invoke<void>("preview_tts_dictionary", {
        input: {
          text: sample,
          original: selectedSource === "custom" ? selected : null,
          entry:
            written.trim() && (skip || spoken.trim())
              ? { written: written.trim(), spoken: skip ? "" : spoken.trim() }
              : null,
        },
      });
      setPlayed(true);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  const filtered = entries.filter((entry) =>
    `${entry.written} ${entry.spoken || "読み飛ばす"}`
      .toLocaleLowerCase()
      .includes(search.toLocaleLowerCase()),
  );
  const candidate = written.trim();
  const selectedUnchanged = selected === candidate && selectedSource !== null;
  const customDuplicate = !selectedUnchanged && entries.some((entry) => entry.written === candidate);
  const checkingDuplicate = Boolean(candidate && !selectedUnchanged && !customDuplicate && checkedWritten?.written !== candidate);
  const duplicate = customDuplicate ? "その表記はカスタム登録語に登録されています。一覧から選択して編集してください。" :
    !selectedUnchanged && checkedWritten?.written === candidate && checkedWritten.existing
      ? `その表記は${checkedWritten.existing.source === "preset" ? "既定語彙" : "カスタム登録語"}に登録されています。検索して選択すると編集できます。`
      : "";
  const shownPresets = presets?.entries ?? [];

  return (
    <section className="tts-dictionary-page" aria-label="TTS辞書">
      <header>
        <h1>TTS辞書</h1>
        <p>言葉の読み方を登録し、実際の音声で確認できます。</p>
      </header>
      <div className="tts-dictionary-layout">
        <aside className="tts-dictionary-list">
          <div className="tts-dictionary-list-tools">
            <input
              aria-label="辞書を検索"
              placeholder="カスタム語・既定語彙を検索"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
            />
            <button type="button" onClick={() => select(null)}>
              ＋ 新規登録
            </button>
          </div>
          <p>
            カスタム登録語一覧 <span>{entries.length}件</span>
          </p>
          {filtered.map((entry) => (
            <button
              className={selected === entry.written ? "selected" : ""}
              type="button"
              key={entry.written}
              onClick={() => select(entry, "custom")}
            >
              <strong>{entry.written}</strong>
              <span>{entry.spoken || "読み飛ばす"}</span>
              <span aria-hidden="true">›</span>
            </button>
          ))}
          {entries.length === 0 && <p className="tts-dictionary-empty">登録語はまだありません。</p>}
          <p className="tts-dictionary-preset-heading">
            既定語彙 <span>{presets ? `${presets.total.toLocaleString()}語` : "読み込み中…"}</span>
          </p>
          <div className="tts-dictionary-presets">
              {shownPresets.map((entry) => (
                <button
                  className={selected === entry.written && selectedSource === "preset" ? "selected" : ""}
                  type="button"
                  key={entry.written}
                  onClick={() => select(entry, "preset")}
                >
                  <strong>{entry.written}</strong>
                  <span>{entry.spoken}</span>
                  <small>プリセット</small>
                </button>
              ))}
              {!presets && <p>既定語彙を読み込み中…</p>}
              {presets && shownPresets.length === 0 && <p>一致する既定語彙はありません。</p>}
              {presets && shownPresets.length === 100 && (
                <p>{search.trim() ? "検索結果の先頭100件を表示しています。検索語を絞ると残りも探せます。" : "先頭100件を表示しています。残りは検索で探せます。"}</p>
              )}
            </div>
          <p className="tts-dictionary-source">
            既定の読み方は{" "}
            <a
              href="https://github.com/WorksApplications/SudachiDict"
              target="_blank"
              rel="noopener noreferrer"
            >
              SudachiDict
            </a>{" "}
            と{" "}
            <a
              href="https://www.edrdg.org/wiki/JMdict-EDICT_Dictionary_Project.html"
              target="_blank"
              rel="noopener noreferrer"
            >
              JMdict
            </a>{" "}
            に基づきます。カスタム登録語が優先されます。
          </p>
        </aside>
        <article className="tts-dictionary-editor">
          <h2>読み方を編集</h2>
          <label>
            文字としての表記
            <input
              aria-invalid={Boolean(duplicate)}
              aria-describedby={duplicate ? "tts-dictionary-duplicate" : undefined}
              value={written}
              maxLength={200}
              onChange={(event) => {
                const next = event.target.value;
                if (!sample && next.trim()) setSample(`${next.trim()}に相談してみましょう。`);
                setWritten(next);
              }}
            />
          </label>
          {duplicate && <p id="tts-dictionary-duplicate" className="tts-dictionary-error" role="alert">{duplicate}</p>}
          <label>
            読み上げ方
            <input
              value={spoken}
              maxLength={400}
              disabled={skip}
              placeholder={skip ? "この表記を読み飛ばします" : undefined}
              onChange={(event) => setSpoken(event.target.value)}
            />
          </label>
          <label className="tts-dictionary-skip">
            <input
              type="checkbox"
              checked={skip}
              onChange={(event) => setSkip(event.target.checked)}
            />
            読み飛ばす（音声にしない）
          </label>
          <p className="tts-dictionary-hint">会話中の表記は変えず、読み上げ時だけ置き換えます。</p>
          {selectedSource === "preset" && (
            <p className="tts-dictionary-hint">プリセットを保存するとカスタム登録語になり、既定の読みより優先されます。</p>
          )}
          <div className="tts-dictionary-preview">
            <h3>試し読み</h3>
            <textarea
              aria-label="試し読みの文"
              value={sample}
              maxLength={1000}
              onChange={(event) => setSample(event.target.value)}
            />
            <button
              type="button"
              disabled={busy || !sample.trim() || !written.trim() || (!skip && !spoken.trim())}
              onClick={() => void preview()}
            >
              {busy ? "音声を生成中…" : "🔊 この文を読み上げる"}
            </button>
            <small>現在のTTS設定で生成します。保存前の読み方も試せます。</small>
            {played && (
              <small role="status">再生しました。読み方を変えてもう一度確認できます。</small>
            )}
          </div>
          {error && (
            <p className="tts-dictionary-error" role="alert">
              {error}
            </p>
          )}
          <div className="tts-dictionary-actions">
            {selected && selectedSource === "custom" && (
              <button type="button" disabled={busy} onClick={() => void remove()}>
                削除
              </button>
            )}
            <button
              type="button"
              className="primary"
              disabled={busy || checkingDuplicate || Boolean(duplicate) || !written.trim() || (!skip && !spoken.trim())}
              onClick={() => void save()}
            >
              保存
            </button>
          </div>
        </article>
      </div>
    </section>
  );
}
