import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import "./ttsDictionaryPage.css";

type Entry = { written: string; spoken: string };
type Draft = Entry & { original: string | null };
const NEW_ROW = "__new__";
const PAGE_SIZE = 90;

export function TtsDictionaryPage() {
  const [entries, setEntries] = useState<Entry[]>([]);
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [adding, setAdding] = useState(false);
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(0);
  const [focusedReading, setFocusedReading] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const newInput = useRef<HTMLInputElement>(null);
  const pending = useRef(false);
  const saveTail = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    let active = true;
    void invoke<Entry[]>("list_tts_dictionary")
      .then((items) => {
        if (active) setEntries(items);
      })
      .catch((cause) => {
        if (active) setError(String(cause));
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    if (adding) newInput.current?.focus();
  }, [adding]);

  function draftFor(key: string, entry?: Entry): Draft {
    return (
      drafts[key] ?? {
        original: entry?.written ?? null,
        written: entry?.written ?? "",
        spoken: entry?.spoken ?? "",
      }
    );
  }

  function edit(key: string, field: "written" | "spoken", value: string, entry?: Entry) {
    const current = draftFor(key, entry);
    setDrafts((previous) => ({ ...previous, [key]: { ...current, [field]: value } }));
    setError("");
  }

  async function save(key: string, entry?: Entry) {
    const draft = drafts[key];
    if (!draft) return;
    const written = draft.written.trim();
    const spoken = draft.spoken.trim();
    if (!written && !spoken && key === NEW_ROW) return;
    if (!written || (!spoken && key === NEW_ROW)) {
      setError("文字と読み方を入力してください。");
      return;
    }
    if (entries.some((item) => item.written === written && item.written !== draft.original)) {
      setError(`「${written}」は登録済みです。`);
      return;
    }
    if (draft.original === written && entry?.spoken === spoken) {
      setDrafts((previous) => {
        const next = { ...previous };
        delete next[key];
        return next;
      });
      return;
    }
    const previous = saveTail.current;
    let release!: () => void;
    saveTail.current = new Promise<void>((resolve) => {
      release = resolve;
    });
    await previous;
    pending.current = true;
    setBusy(true);
    try {
      const saved = await invoke<Entry[]>("save_tts_dictionary_entry", {
        input: { original: draft.original, presetOverride: null, entry: { written, spoken } },
      });
      setEntries(saved);
      setDrafts((previous) => {
        const next = { ...previous };
        delete next[key];
        return next;
      });
      if (key === NEW_ROW) setAdding(false);
      if (focusedReading === key) setFocusedReading(written);
      setError("");
    } catch (cause) {
      setError(String(cause));
    } finally {
      pending.current = false;
      setBusy(false);
      release();
    }
  }

  async function remove(entry: Entry) {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    try {
      setEntries(await invoke<Entry[]>("delete_tts_dictionary_entry", { written: entry.written }));
      setDrafts((previous) => {
        const next = { ...previous };
        delete next[entry.written];
        return next;
      });
      if (focusedReading === entry.written) setFocusedReading(null);
      setError("");
    } catch (cause) {
      setError(String(cause));
    } finally {
      pending.current = false;
      setBusy(false);
    }
  }

  async function preview() {
    if (!focusedReading || pending.current) return;
    const entry = entries.find((item) => item.written === focusedReading);
    const reading = draftFor(focusedReading, entry).spoken.trim();
    if (!reading) return;
    pending.current = true;
    setBusy(true);
    try {
      await invoke<void>("preview_tts_dictionary", {
        input: { text: reading, original: null, entry: null, raw: true },
      });
      setError("");
    } catch (cause) {
      setError(String(cause));
    } finally {
      pending.current = false;
      setBusy(false);
      if ((document.activeElement as HTMLElement | null)?.dataset.ttsPreview === "true") {
        void save(focusedReading, entry);
      }
    }
  }

  const filtered = entries.filter((entry) =>
    `${entry.written} ${entry.spoken}`.toLocaleLowerCase().includes(search.toLocaleLowerCase()),
  );
  const rows: Array<{ key: string; entry?: Entry }> = filtered.map((entry) => ({
    key: entry.written,
    entry,
  }));
  if (adding && !search) rows.push({ key: NEW_ROW });
  const pageCount = Math.max(1, Math.ceil(rows.length / PAGE_SIZE));
  const currentPage = Math.min(page, pageCount - 1);
  const visibleRows = rows.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE);
  const perTable = Math.max(1, Math.ceil(visibleRows.length / 3));
  const selected = focusedReading
    ? draftFor(
        focusedReading,
        entries.find((item) => item.written === focusedReading),
      )
    : null;

  return (
    <section className="tts-dictionary-page" aria-label="TTS辞書">
      <h1>TTS辞書</h1>
      <div className="tts-dictionary-toolbar">
        <input
          aria-label="辞書を検索"
          placeholder="文字・読み方を検索"
          value={search}
          onChange={(event) => {
            setSearch(event.target.value);
            setPage(0);
          }}
        />
        <button
          type="button"
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => {
            setSearch("");
            setAdding(true);
            setPage(Math.floor(entries.length / PAGE_SIZE));
          }}
          disabled={adding || busy}
        >
          ＋ 行を追加
        </button>
        <button
          type="button"
          data-tts-preview="true"
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => void preview()}
          disabled={busy || !selected?.spoken.trim()}
        >
          ▶ 選択セルを試し読み
        </button>
        <span>選択中の「読み方」セルだけを再生</span>
      </div>
      {error && (
        <p className="tts-dictionary-error" role="alert">
          {error}
        </p>
      )}
      <div className="tts-dictionary-tables">
        {Array.from({ length: 3 }, (_, index) => (
          <table key={index} aria-label={`辞書 ${index + 1}`}>
            <thead>
              <tr>
                <th>文字</th>
                <th>読み方</th>
                <th>削除</th>
              </tr>
            </thead>
            <tbody>
              {visibleRows.slice(index * perTable, (index + 1) * perTable).map(({ key, entry }) => {
                const draft = draftFor(key, entry);
                return (
                  <tr key={key}>
                    <td>
                      <input
                        ref={key === NEW_ROW ? newInput : undefined}
                        aria-label={`${entry?.written ?? "新規"}の文字`}
                        value={draft.written}
                        maxLength={200}
                        onFocus={() => setFocusedReading(null)}
                        onChange={(event) => edit(key, "written", event.target.value, entry)}
                        onBlur={(event) => {
                          if (
                            key === NEW_ROW &&
                            (event.relatedTarget as HTMLElement | null)?.getAttribute(
                              "aria-label",
                            ) === "新規の読み方"
                          )
                            return;
                          void save(key, entry);
                        }}
                        onKeyDown={(event) => {
                          if (event.key === "Enter") event.currentTarget.blur();
                        }}
                      />
                    </td>
                    <td>
                      <input
                        aria-label={`${entry?.written ?? "新規"}の読み方`}
                        value={draft.spoken}
                        maxLength={400}
                        onFocus={() => setFocusedReading(key)}
                        onChange={(event) => edit(key, "spoken", event.target.value, entry)}
                        onBlur={(event) => {
                          if (
                            (event.relatedTarget as HTMLElement | null)?.dataset.ttsPreview !==
                            "true"
                          )
                            void save(key, entry);
                        }}
                        onKeyDown={(event) => {
                          if (event.key === "Enter") event.currentTarget.blur();
                        }}
                      />
                    </td>
                    <td>
                      <button
                        type="button"
                        aria-label={`${entry?.written ?? "新規行"}を削除`}
                        disabled={busy}
                        onMouseDown={(event) => event.preventDefault()}
                        onClick={() => {
                          if (entry) void remove(entry);
                          else {
                            setAdding(false);
                            setError("");
                            setDrafts((previous) => {
                              const next = { ...previous };
                              delete next[NEW_ROW];
                              return next;
                            });
                          }
                        }}
                      >
                        ×
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        ))}
      </div>
      <div className="tts-dictionary-count">
        <span>{filtered.length}件</span>
        {pageCount > 1 && (
          <nav aria-label="辞書のページ">
            <button
              type="button"
              disabled={currentPage === 0}
              onClick={() => setPage(currentPage - 1)}
            >
              前へ
            </button>
            <span>
              {currentPage + 1} / {pageCount}
            </span>
            <button
              type="button"
              disabled={currentPage === pageCount - 1}
              onClick={() => setPage(currentPage + 1)}
            >
              次へ
            </button>
          </nav>
        )}
      </div>
    </section>
  );
}
