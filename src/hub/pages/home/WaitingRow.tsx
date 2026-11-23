import { ChevronDown, ChevronUp, Clock } from "lucide-react";
import { useState } from "react";
import { api, type HistoryItem } from "../../../shared/api";
import { errorText, useToast } from "../../components";
import HistoryRow from "./HistoryRow";
import { waitingSummary, type KeyState } from "./helpers";

/**
 * Recordings that could not be processed yet (no key, no internet...) are kept safe, but
 * shown as one compact row instead of one row each.
 */
export default function WaitingRow({
  items,
  keys,
  onOpen,
  onChanged,
}: {
  items: HistoryItem[];
  keys: KeyState | null;
  onOpen: (id: string) => void;
  onChanged: () => void;
}) {
  const toast = useToast();
  const [expanded, setExpanded] = useState(false);
  const [busy, setBusy] = useState(false);
  if (items.length === 0) return null;
  const { text } = waitingSummary(items, keys);

  async function retryAll() {
    setBusy(true);
    try {
      const r = await api.retryWaiting();
      toast(r.stillWaiting === 0 ? "All done. Your text is in History." : `${r.done} done, ${r.stillWaiting} still waiting`);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
      onChanged();
    }
  }

  return (
    <section className="home-waiting" aria-label="Recordings waiting to be processed">
      <div className="card home-waiting-bar">
        <Clock size={18} className="home-waiting-icon" aria-hidden />
        <span className="home-waiting-text">{text}</span>
        <button className="btn btn-dark btn-sm" disabled={busy} onClick={retryAll}>
          {busy ? "Trying…" : "Try all again"}
        </button>
        <button className="btn btn-soft btn-sm" onClick={() => setExpanded((v) => !v)} aria-expanded={expanded}>
          {expanded ? "Hide them" : "Show them"}
          {expanded ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
        </button>
      </div>
      {expanded && (
        <div className="list home-waiting-list">
          {items.map((it) => (
            <HistoryRow key={it.id} item={it} onOpen={() => onOpen(it.id)} />
          ))}
        </div>
      )}
    </section>
  );
}
