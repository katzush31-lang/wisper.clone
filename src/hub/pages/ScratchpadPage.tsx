import { Mic, NotebookPen, Pin, Plus, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, events, type Note } from "../../shared/api";
import { ConfirmDialog, EmptyState, SearchBox, errorText, useToast } from "../components";
import { autoTitle, insertAtCaret, relativeTime } from "./library/helpers";
import "./library/library.css";

const SAVE_DELAY_MS = 600;

type SaveState = "idle" | "saving" | "saved" | "error";

export default function ScratchpadPage() {
  const toast = useToast();
  const [notes, setNotes] = useState<Note[]>([]);
  const [query, setQuery] = useState("");
  const [current, setCurrent] = useState<Note | null>(null);
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [dictating, setDictating] = useState(false);

  const currentRef = useRef<Note | null>(null);
  const dirty = useRef(false);
  const timer = useRef<number | undefined>(undefined);
  const chain = useRef<Promise<unknown>>(Promise.resolve());
  const bodyRef = useRef<HTMLTextAreaElement>(null);
  const queryRef = useRef(query);
  queryRef.current = query;

  const loadNotes = useCallback(() => {
    api
      .listNotes(queryRef.current.trim() || undefined)
      .then(setNotes)
      .catch(() => setNotes([]));
  }, []);

  useEffect(() => {
    const t = window.setTimeout(loadNotes, 150);
    return () => window.clearTimeout(t);
  }, [query, loadNotes]);

  const setNote = (n: Note | null) => {
    currentRef.current = n;
    setCurrent(n);
  };

  /** Saves the current note now (serialised so a new note is never created twice). */
  const flush = useCallback((): Promise<unknown> => {
    window.clearTimeout(timer.current);
    if (!dirty.current || !currentRef.current) return chain.current;
    dirty.current = false;
    chain.current = chain.current.then(async () => {
      const n = currentRef.current;
      if (!n) return;
      const toSave: Note = {
        ...n,
        title: n.title.trim() ? n.title : autoTitle(n.body),
        updatedAt: Date.now(),
      };
      setSaveState("saving");
      try {
        const id = await api.saveNote(toSave);
        if (currentRef.current && currentRef.current.id === n.id) {
          // keep what the user typed meanwhile; only adopt the new id and times
          currentRef.current = { ...currentRef.current, id, updatedAt: toSave.updatedAt };
          setCurrent(currentRef.current);
        }
        setSaveState("saved");
        loadNotes();
      } catch (e) {
        setSaveState("error");
        toast(errorText(e), "error");
      }
    });
    return chain.current;
  }, [loadNotes, toast]);

  const edit = (patch: Partial<Note>) => {
    const n = currentRef.current;
    if (!n) return;
    setNote({ ...n, ...patch });
    dirty.current = true;
    setSaveState("idle");
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(flush, SAVE_DELAY_MS);
  };

  // Save whatever is pending when leaving the page.
  useEffect(() => () => void flush(), [flush]);

  const select = async (n: Note) => {
    await flush();
    setSaveState("idle");
    setNote(n);
  };

  const newNote = async () => {
    await flush();
    const now = Date.now();
    setSaveState("idle");
    setNote({ id: 0, title: "", body: "", pinned: false, createdAt: now, updatedAt: now });
    window.setTimeout(() => bodyRef.current?.focus(), 0);
  };

  // Dictation text arriving from the backend goes into the open note.
  useEffect(() => {
    const un = events.scratchpadText((text) => {
      setDictating(false);
      let n = currentRef.current;
      if (!n) {
        const now = Date.now();
        n = { id: 0, title: "", body: "", pinned: false, createdAt: now, updatedAt: now };
        currentRef.current = n;
      }
      const el = bodyRef.current;
      const focused = el && document.activeElement === el;
      const r = insertAtCaret(n.body, text, focused ? el.selectionStart : null, focused ? el.selectionEnd : null);
      setNote(n);
      edit({ body: r.value });
      window.setTimeout(() => {
        const e2 = bodyRef.current;
        if (e2) {
          e2.focus();
          e2.setSelectionRange(r.caret, r.caret);
        }
      }, 0);
      void flush();
    });
    return () => {
      un.then((f) => f()).catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [flush]);

  const dictate = async () => {
    try {
      setDictating(true);
      await api.dictateIntoScratchpad();
    } catch (e) {
      setDictating(false);
      toast(errorText(e), "error");
    }
  };

  const remove = async () => {
    const n = currentRef.current;
    setConfirmDelete(false);
    window.clearTimeout(timer.current);
    dirty.current = false;
    setNote(null);
    if (n && n.id !== 0) {
      try {
        await api.deleteNote(n.id);
        toast("Note deleted");
      } catch (e) {
        toast(errorText(e), "error");
      }
    }
    loadNotes();
  };

  const sorted = useMemo(
    () => [...notes].sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.updatedAt - a.updatedAt),
    [notes],
  );

  return (
    <div className="pad">
      <div className="pad-left">
        <div className="pad-left-head">
          <h1>Scratchpad</h1>
          <SearchBox value={query} onChange={setQuery} placeholder="Search notes" />
          <button className="btn btn-dark" onClick={newNote}>
            <Plus size={16} /> New note
          </button>
        </div>
        <div className="pad-list">
          {sorted.length === 0 && (
            <div className="muted" style={{ padding: "16px 12px", fontSize: 13 }}>
              {query ? "No notes match your search." : "No notes yet."}
            </div>
          )}
          {sorted.map((n) => {
            const active = current?.id === n.id && n.id !== 0;
            return (
              <button key={n.id} className={`pad-item ${active ? "active" : ""}`} onClick={() => select(n)}>
                <div className="pad-item-title">
                  {n.pinned && <Pin size={12} />}
                  <span>{(active ? current?.title : n.title) || (active && current?.body ? autoTitle(current.body) : "") || "Untitled"}</span>
                </div>
                <div className="pad-item-time">{relativeTime(n.updatedAt)}</div>
              </button>
            );
          })}
        </div>
      </div>

      {current ? (
        <div className="pad-right">
          <div className="pad-bar">
            <input
              className="pad-title"
              type="text"
              placeholder="Untitled"
              aria-label="Note title"
              value={current.title}
              onChange={(e) => edit({ title: e.target.value })}
            />
            <span className="pad-status" aria-live="polite">
              {saveState === "saving" ? "Saving…" : saveState === "saved" ? "Saved" : saveState === "error" ? "Not saved" : ""}
            </span>
            <button
              className="btn btn-soft btn-sm"
              onClick={dictate}
              disabled={dictating}
              title="Speak, and your words are added to this note"
            >
              <Mic size={14} /> {dictating ? "Listening…" : "Dictate"}
            </button>
            <button
              className="icon-btn"
              onClick={() => edit({ pinned: !current.pinned })}
              aria-label={current.pinned ? "Unpin note" : "Pin note to the top"}
              title={current.pinned ? "Unpin" : "Pin to the top of the list"}
              style={current.pinned ? { color: "var(--cta)" } : undefined}
            >
              <Pin size={17} />
            </button>
            <button className="icon-btn" onClick={() => setConfirmDelete(true)} aria-label="Delete note" title="Delete note">
              <Trash2 size={17} />
            </button>
          </div>
          <textarea
            ref={bodyRef}
            className="pad-body"
            placeholder="Start typing, or press Dictate and talk…"
            aria-label="Note text"
            value={current.body}
            onChange={(e) => edit({ body: e.target.value })}
          />
        </div>
      ) : (
        <div className="pad-right" style={{ justifyContent: "center" }}>
          <EmptyState icon={<NotebookPen size={30} />} title={notes.length ? "Pick a note" : "Your notes live here"}>
            A quick place to jot things down or dictate ideas. Everything saves by itself.
            <div style={{ marginTop: 16 }}>
              <button className="btn btn-dark" onClick={newNote}>
                <Plus size={16} /> New note
              </button>
            </div>
          </EmptyState>
        </div>
      )}

      <ConfirmDialog
        open={confirmDelete}
        title="Delete this note?"
        message="This can’t be undone."
        onConfirm={remove}
        onCancel={() => setConfirmDelete(false)}
      />
    </div>
  );
}
