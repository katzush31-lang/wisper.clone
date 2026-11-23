import { Pencil, Scissors, Trash2, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { api, events, type Snippet } from "../../shared/api";
import { ConfirmDialog, EmptyState, Modal, SearchBox, errorText, useToast } from "../components";
import { previewSnippetsJson, truncate } from "./library/helpers";
import "./library/library.css";

type Tab = "all" | "used";

const EXAMPLES = [
  { say: "my email address", text: "you@example.com", italic: true },
  { say: "my address", text: "12 High Street, London", italic: true },
  { say: "thanks reply", text: "Thanks so much — I'll get back to you shortly.", italic: true },
];

export default function SnippetsPage() {
  const toast = useToast();
  const [items, setItems] = useState<Snippet[]>([]);
  const [tab, setTab] = useState<Tab>("all");
  const [query, setQuery] = useState("");
  const [editing, setEditing] = useState<Snippet | null>(null);
  const [deleting, setDeleting] = useState<Snippet | null>(null);
  const [importing, setImporting] = useState(false);
  const [banner, setBanner] = useState(true);

  const refresh = useCallback(() => {
    api.listSnippets().then(setItems).catch(() => setItems([]));
  }, []);
  useEffect(() => {
    refresh();
    const un = events.snippetsChanged(refresh);
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  }, [refresh]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return items
      .filter((s) => (tab === "used" ? s.uses > 0 : true))
      .filter((s) => !q || s.trigger.toLowerCase().includes(q) || s.expansion.toLowerCase().includes(q))
      .sort((a, b) => (tab === "used" ? b.uses - a.uses : 0) || a.trigger.localeCompare(b.trigger));
  }, [items, tab, query]);

  const add = () => setEditing({ id: 0, trigger: "", expansion: "", uses: 0 });

  const remove = async () => {
    if (!deleting) return;
    try {
      await api.deleteSnippet(deleting.id);
      toast("Snippet deleted");
    } catch (e) {
      toast(errorText(e), "error");
    }
    setDeleting(null);
    refresh();
  };

  return (
    <div className="page page-narrow">
      <div className="page-header">
        <h1 className="page-title">Snippets</h1>
        <div className="lib-actions">
          <button className="btn btn-soft" onClick={() => setImporting(true)}>
            Import
          </button>
          <button className="btn btn-dark" onClick={add}>
            Add new
          </button>
        </div>
      </div>

      <div className="lib-tabs-wrap">
        <div className="tabs">
          <button className={`tab ${tab === "all" ? "active" : ""}`} onClick={() => setTab("all")}>
            All
          </button>
          <button className={`tab ${tab === "used" ? "active" : ""}`} onClick={() => setTab("used")}>
            Most used
          </button>
        </div>
        <SearchBox value={query} onChange={setQuery} placeholder="Search snippets" />
      </div>

      {banner && (
        <div className="hero lib-banner">
          <button className="icon-btn lib-hero-close" onClick={() => setBanner(false)} aria-label="Hide this banner">
            <X size={16} />
          </button>
          <h2>
            Stop re-typing the same things.
          </h2>
          <p className="lib-hero-text">
            Save text you type often — an email, an address, a reply — then say a short phrase to drop it in
            instantly.
          </p>
          <div className="lib-hero-chips">
            {EXAMPLES.map((x) => (
              <div className="lib-chip-line" key={x.say}>
                <span className={`lib-chip ${x.italic ? "italic" : ""}`}>“{x.say}”</span>
                <span aria-hidden>→</span>
                <span className="lib-chip">{truncate(x.text, 60)}</span>
              </div>
            ))}
          </div>
          <button className="btn btn-light" onClick={add}>
            Add new snippet
          </button>
        </div>
      )}

      {shown.length === 0 ? (
        <EmptyState icon={<Scissors size={28} />} title={query ? "No snippets match your search" : "No snippets yet"}>
          {query
            ? "Try a different word."
            : tab === "used"
              ? "Snippets you use will show up here."
              : "Add your first one with “Add new”."}
        </EmptyState>
      ) : (
        <div className="list">
          {shown.map((s) => (
            <div className="list-row lib-row-center" key={s.id}>
              <div className="lib-row-main lib-snippet-line">
                <span className="lib-trigger">{s.trigger}</span>
                <span aria-hidden className="faint">
                  →
                </span>
                <span className="lib-oneline">{truncate(s.expansion, 140)}</span>
              </div>
              {s.uses > 0 && (
                <span className="muted" style={{ fontSize: 12.5, whiteSpace: "nowrap" }}>
                  Used {s.uses} {s.uses === 1 ? "time" : "times"}
                </span>
              )}
              <div className="lib-row-actions">
                <button className="icon-btn" onClick={() => setEditing(s)} aria-label={`Edit ${s.trigger}`}>
                  <Pencil size={16} />
                </button>
                <button className="icon-btn" onClick={() => setDeleting(s)} aria-label={`Delete ${s.trigger}`}>
                  <Trash2 size={16} />
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      {editing && (
        <SnippetModal
          snippet={editing}
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
        title="Delete this snippet?"
        message={`“${deleting?.trigger}” will no longer expand.`}
        onConfirm={remove}
        onCancel={() => setDeleting(null)}
      />
    </div>
  );
}

function SnippetModal({
  snippet,
  onClose,
  onSaved,
}: {
  snippet: Snippet;
  onClose: () => void;
  onSaved: () => void;
}) {
  const toast = useToast();
  const [trigger, setTrigger] = useState(snippet.trigger);
  const [expansion, setExpansion] = useState(snippet.expansion);
  const [busy, setBusy] = useState(false);
  const t = trigger.trim();
  const valid = t.length > 0 && t.length <= 60 && expansion.trim().length > 0;

  const save = async () => {
    if (!valid) return;
    setBusy(true);
    try {
      await api.saveSnippet({ ...snippet, trigger: t, expansion });
      toast("Snippet saved");
      onSaved();
    } catch (e) {
      toast(errorText(e), "error");
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      title={snippet.id === 0 ? "Add a snippet" : "Edit snippet"}
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-soft" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-dark" onClick={save} disabled={!valid || busy}>
            Save
          </button>
        </>
      }
    >
      <div className="lib-field">
        <label className="lib-label" htmlFor="snip-trigger">
          Trigger phrase
        </label>
        <p className="lib-help">What you’ll say, e.g. “my email address”.</p>
        <input
          id="snip-trigger"
          className="input"
          type="text"
          autoFocus
          maxLength={60}
          value={trigger}
          onChange={(e) => setTrigger(e.target.value)}
        />
      </div>
      <div className="lib-field">
        <label className="lib-label" htmlFor="snip-text">
          Text to insert
        </label>
        <p className="lib-help">This is typed out exactly as written. It can be several lines.</p>
        <textarea
          id="snip-text"
          className="textarea"
          rows={6}
          value={expansion}
          onChange={(e) => setExpansion(e.target.value)}
        />
      </div>
    </Modal>
  );
}

function ImportModal({ open, onClose, onDone }: { open: boolean; onClose: () => void; onDone: () => void }) {
  const toast = useToast();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const count = previewSnippetsJson(text);

  const run = async () => {
    setBusy(true);
    try {
      const r = await api.importSnippetsJson(text);
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
      title="Import snippets"
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-soft" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-dark" onClick={run} disabled={!count || busy}>
            Import
          </button>
        </>
      }
    >
      <p className="lib-help">
        Paste your snippets as a list. Wispr Flow’s export works as it is (each item has a “name” and a “text”).
        Snippets you already have are skipped.
      </p>
      <textarea
        className="textarea lib-code"
        rows={8}
        autoFocus
        placeholder={'[{"name": "my email", "text": "you@example.com"}]'}
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      {text.trim() && (
        <div className="lib-preview">
          {count === null
            ? "That doesn’t look like a snippet list yet."
            : `Found ${count} ${count === 1 ? "snippet" : "snippets"}.`}
        </div>
      )}
    </Modal>
  );
}
