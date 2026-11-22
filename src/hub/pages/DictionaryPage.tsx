import { BookA, Pencil, Star, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { api, events, type DictEntry, type Suggestion } from "../../shared/api";
import { ConfirmDialog, EmptyState, Modal, SearchBox, Switch, errorText, useToast } from "../components";
import { joinList, previewDictionaryCsv, splitList } from "./library/helpers";
import "./library/library.css";

type Tab = "all" | "mine" | "builtin";

const blank = (): DictEntry => ({
  id: 0,
  word: "",
  replace: [],
  soundsLike: [],
  source: "user",
  starred: false,
  keyword: true,
  uses: 0,
});

export default function DictionaryPage() {
  const toast = useToast();
  const [entries, setEntries] = useState<DictEntry[]>([]);
  const [suggestions, setSuggestions] = useState<[number, Suggestion][]>([]);
  const [tab, setTab] = useState<Tab>("all");
  const [query, setQuery] = useState("");
  const [editing, setEditing] = useState<DictEntry | null>(null);
  const [deleting, setDeleting] = useState<DictEntry | null>(null);
  const [importing, setImporting] = useState(false);

  const refresh = useCallback(() => {
    api.listDictionary().then(setEntries).catch(() => setEntries([]));
  }, []);
  const refreshSuggestions = useCallback(() => {
    api.listSuggestions().then(setSuggestions).catch(() => setSuggestions([]));
  }, []);

  useEffect(() => {
    refresh();
    refreshSuggestions();
    const un = events.suggestionsChanged(() => {
      refreshSuggestions();
      refresh();
    });
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  }, [refresh, refreshSuggestions]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return entries
      .filter((e) => (tab === "all" ? true : tab === "mine" ? e.source !== "builtin" : e.source === "builtin"))
      .filter(
        (e) =>
          !q ||
          [e.word, ...e.replace, ...e.soundsLike].some((s) => s.toLowerCase().includes(q)),
      )
      .sort((a, b) => Number(b.starred) - Number(a.starred) || a.word.localeCompare(b.word));
  }, [entries, tab, query]);

  const resolve = async (id: number, accept: boolean) => {
    try {
      await api.resolveSuggestion(id, accept);
      toast(accept ? "Added to your dictionary" : "Dismissed");
    } catch (e) {
      toast(errorText(e), "error");
    }
    refreshSuggestions();
    refresh();
  };

  const toggleStar = async (e: DictEntry) => {
    const next = { ...e, starred: !e.starred };
    setEntries((list) => list.map((x) => (x.id === e.id ? next : x)));
    try {
      await api.updateDictionaryEntry(next);
    } catch (err) {
      toast(errorText(err), "error");
      refresh();
    }
  };

  const remove = async () => {
    if (!deleting) return;
    try {
      await api.deleteDictionaryEntry(deleting.id);
      toast("Word deleted");
    } catch (e) {
      toast(errorText(e), "error");
    }
    setDeleting(null);
    refresh();
  };

  return (
    <div className="page page-narrow">
      <div className="page-header">
        <h1 className="page-title">Dictionary</h1>
        <div className="lib-actions">
          <button className="btn btn-dark" onClick={() => setImporting(true)}>
            Import
          </button>
          <button className="btn btn-dark" onClick={() => setEditing(blank())}>
            Add new
          </button>
        </div>
      </div>

      <div className="lib-tabs-wrap">
        <div className="tabs">
          {(
            [
              ["all", "All"],
              ["mine", "Mine"],
              ["builtin", "Built-in Hebrew & Yiddish"],
            ] as [Tab, string][]
          ).map(([id, label]) => (
            <button key={id} className={`tab ${tab === id ? "active" : ""}`} onClick={() => setTab(id)}>
              {label}
            </button>
          ))}
        </div>
        <SearchBox value={query} onChange={setQuery} placeholder="Search words" />
      </div>

      <div className="card lib-banner-plain">
        <BookA size={20} />
        <p>
          Words Wisper should always spell your way. Hebrew and Yiddish words come pre-loaded in Ashkenazi spelling.
        </p>
      </div>

      {suggestions.length > 0 && (
        <div className="card lib-suggest">
          <h3>Suggestions</h3>
          {suggestions.map(([id, s]) => (
            <div className="lib-suggest-row" key={id}>
              <span>
                We noticed “{s.variants.join("”, “")}” and “{s.word}” — use “{s.word}” every time?
              </span>
              <button className="btn btn-dark btn-sm" onClick={() => resolve(id, true)}>
                Add
              </button>
              <button className="btn btn-soft btn-sm" onClick={() => resolve(id, false)}>
                Dismiss
              </button>
            </div>
          ))}
        </div>
      )}

      {shown.length === 0 ? (
        <EmptyState icon={<BookA size={28} />} title={query ? "No words match your search" : "Nothing here yet"}>
          {query
            ? "Try a different spelling."
            : "Add names, places and special words so Wisper always gets them right."}
        </EmptyState>
      ) : (
        <div className="list">
          {shown.map((e) => (
            <div className="list-row lib-row-center" key={e.id}>
              <button
                className={`icon-btn lib-star ${e.starred ? "on" : ""}`}
                onClick={() => toggleStar(e)}
                aria-label={e.starred ? "Remove star" : "Star this word"}
                title={e.starred ? "Starred" : "Star this word to keep it handy"}
              >
                <Star size={17} />
              </button>
              <div className="lib-row-main">
                <div className="lib-row-title">
                  {e.word}
                  {e.keyword && <span className="badge lib-badge">Hint to speech recognition</span>}
                </div>
                {e.replace.length > 0 && <div className="lib-row-sub">Also fixes: {e.replace.join(", ")}</div>}
                {e.soundsLike.length > 0 && <div className="lib-row-sub">Sounds like: {e.soundsLike.join(", ")}</div>}
              </div>
              <div className="lib-row-actions">
                <button className="icon-btn" onClick={() => setEditing(e)} aria-label={`Edit ${e.word}`}>
                  <Pencil size={16} />
                </button>
                <button className="icon-btn" onClick={() => setDeleting(e)} aria-label={`Delete ${e.word}`}>
                  <Trash2 size={16} />
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      {editing && (
        <EntryModal
          entry={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            refresh();
          }}
        />
      )}
      <ImportModal
        open={importing}
        onClose={() => setImporting(false)}
        onDone={() => {
          setImporting(false);
          refresh();
        }}
      />
      <ConfirmDialog
        open={!!deleting}
        title="Delete this word?"
        message={
          deleting?.source === "builtin"
            ? `“${deleting.word}” came pre-loaded. You can add it back any time yourself with “Add new”.`
            : `“${deleting?.word}” will be removed from your dictionary.`
        }
        onConfirm={remove}
        onCancel={() => setDeleting(null)}
      />
    </div>
  );
}

function EntryModal({
  entry,
  onClose,
  onSaved,
}: {
  entry: DictEntry;
  onClose: () => void;
  onSaved: () => void;
}) {
  const toast = useToast();
  const isNew = entry.id === 0;
  const [word, setWord] = useState(entry.word);
  const [replace, setReplace] = useState(joinList(entry.replace));
  const [soundsLike, setSoundsLike] = useState(joinList(entry.soundsLike));
  const [keyword, setKeyword] = useState(entry.keyword);
  const [busy, setBusy] = useState(false);
  const trimmed = word.trim();
  const problem = !trimmed ? "Please type a word or phrase." : trimmed.length > 60 ? "Keep it to 60 characters or fewer." : "";

  const save = async () => {
    if (problem) return;
    setBusy(true);
    const next: DictEntry = {
      ...entry,
      word: trimmed,
      replace: splitList(replace),
      soundsLike: splitList(soundsLike),
      keyword,
    };
    try {
      if (isNew) await api.addDictionaryEntry(next);
      else await api.updateDictionaryEntry(next);
      toast(isNew ? "Word added" : "Saved");
      onSaved();
    } catch (e) {
      toast(errorText(e), "error");
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      title={isNew ? "Add a word" : "Edit word"}
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-soft" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-dark" onClick={save} disabled={!!problem || busy}>
            Save
          </button>
        </>
      }
    >
      <div className="lib-field">
        <label className="lib-label" htmlFor="dict-word">
          Word or phrase
        </label>
        <p className="lib-help">The spelling you want, exactly as it should appear.</p>
        <input
          id="dict-word"
          className="input"
          type="text"
          autoFocus
          maxLength={60}
          placeholder="e.g. zechus"
          value={word}
          onChange={(e) => setWord(e.target.value)}
        />
        {word && problem && <div className="lib-error">{problem}</div>}
      </div>
      <div className="lib-field">
        <label className="lib-label" htmlFor="dict-replace">
          Always replace these spellings
        </label>
        <p className="lib-help">
          Separate with commas. Whenever Wisper hears one of these, it is swapped for your word automatically.
        </p>
        <input
          id="dict-replace"
          className="input"
          type="text"
          placeholder="e.g. zekus, zakhus"
          value={replace}
          onChange={(e) => setReplace(e.target.value)}
        />
      </div>
      <div className="lib-field">
        <label className="lib-label" htmlFor="dict-sounds">
          Might be misheard as
        </label>
        <p className="lib-help">
          Separate with commas. These are only hints to the AI about what the word sounds like — they are never
          blindly replaced.
        </p>
        <input
          id="dict-sounds"
          className="input"
          type="text"
          placeholder="e.g. Zeus, Zachs"
          value={soundsLike}
          onChange={(e) => setSoundsLike(e.target.value)}
        />
      </div>
      <div className="lib-field lib-switch-row">
        <div>
          <div className="lib-label">Help speech recognition with this word</div>
          <p className="lib-help" style={{ margin: 0 }}>
            Tells the speech-to-text step to listen out for this word. Turn off for common words that don’t need it.
          </p>
        </div>
        <Switch checked={keyword} onChange={setKeyword} label="Help speech recognition with this word" />
      </div>
    </Modal>
  );
}

function ImportModal({ open, onClose, onDone }: { open: boolean; onClose: () => void; onDone: () => void }) {
  const toast = useToast();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const preview = previewDictionaryCsv(text);
  const total = preview.words + preview.corrections;

  const run = async () => {
    setBusy(true);
    try {
      const r = await api.importDictionaryCsv(text);
      toast(`Added ${r.added}, skipped ${r.skipped}`);
      setText("");
      onDone();
    } catch (e) {
      toast(errorText(e), "error");
    }
    setBusy(false);
  };

  return (
    <Modal
      open={open}
      title="Import words"
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-soft" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-dark" onClick={run} disabled={total === 0 || busy}>
            Import
          </button>
        </>
      }
    >
      <p className="lib-help">
        Paste one word per line, or “misspelling,correction” to fix a spelling. Words you already have are skipped.
      </p>
      <textarea
        className="textarea lib-code"
        rows={8}
        autoFocus
        placeholder={"Shabbos\nzeus,zechus\nparnasa"}
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      {total > 0 && (
        <div className="lib-preview">
          Found {preview.words} {preview.words === 1 ? "word" : "words"} and {preview.corrections}{" "}
          {preview.corrections === 1 ? "correction" : "corrections"}.
        </div>
      )}
    </Modal>
  );
}
