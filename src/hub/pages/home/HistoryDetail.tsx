import { ChevronDown, ChevronRight, ClipboardCopy, CornerDownLeft, RotateCcw, Trash2, Wand2 } from "lucide-react";
import { useEffect, useState } from "react";
import { api, formatDuration, type HistoryItem, type Mode } from "../../../shared/api";
import { isTauri } from "../../../shared/tauri";
import { ConfirmDialog, Modal, errorText, useToast } from "../../components";
import { itemText, ms, statusInfo } from "./helpers";

export default function HistoryDetail({
  item,
  onClose,
  onChanged,
}: {
  item: HistoryItem;
  onClose: () => void;
  onChanged: () => void;
}) {
  const toast = useToast();
  const [shownText, setShownText] = useState(itemText(item));
  const [showRaw, setShowRaw] = useState(false);
  const [modes, setModes] = useState<Mode[]>([]);
  const [modeId, setModeId] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [audioSrc, setAudioSrc] = useState<string | null>(null);

  useEffect(() => {
    setShownText(itemText(item));
  }, [item]);

  useEffect(() => {
    api
      .listModes()
      .then(setModes)
      .catch(() => {});
  }, []);

  useEffect(() => {
    let alive = true;
    if (!item.audioPath || !isTauri()) return;
    (async () => {
      try {
        const { convertFileSrc } = await import("@tauri-apps/api/core");
        if (alive) setAudioSrc(convertFileSrc(item.audioPath!));
      } catch {
        /* no audio preview */
      }
    })();
    return () => {
      alive = false;
    };
  }, [item.audioPath]);

  async function run<T>(name: string, fn: () => Promise<T>, done?: (r: T) => string | void) {
    setBusy(name);
    try {
      const r = await fn();
      const msg = done?.(r);
      if (msg) toast(msg);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  }

  const status = statusInfo(item);
  // "empty" = judged silent; retrying transcribes it anyway (useful for very quiet speech).
  const canRetry =
    item.status === "failed" || item.status === "queued" || item.status === "cancelled" || (item.status === "empty" && !!item.audioPath);
  const when = new Date(item.createdAt).toLocaleString("en-GB", {
    weekday: "long",
    day: "numeric",
    month: "long",
    hour: "numeric",
    minute: "2-digit",
  });
  const t = item.timings;
  const timing = [
    ms(t?.recordingMs) && `Recording ${ms(t?.recordingMs)}`,
    ms(t?.transcribeWaitMs) && `Waiting after you stopped ${ms(t?.transcribeWaitMs)}`,
    ms(t?.cleanupMs) && `Cleanup ${ms(t?.cleanupMs)}`,
  ].filter(Boolean);
  const facts = [
    item.appName && `in ${item.appName}`,
    item.modeId && `mode: ${modes.find((m) => m.id === item.modeId)?.name ?? item.modeId}`,
    item.durationMs > 0 && formatDuration(item.durationMs),
    `${item.wordCount} ${item.wordCount === 1 ? "word" : "words"}`,
  ].filter(Boolean);

  return (
    <>
      <Modal
        open
        onClose={onClose}
        title={when}
        width={640}
        footer={
          <>
            <button className="btn btn-danger" onClick={() => setConfirmDelete(true)}>
              <Trash2 size={15} /> Delete
            </button>
            <span style={{ flex: 1 }} />
            <button className="btn btn-soft" onClick={onClose}>
              Close
            </button>
          </>
        }
      >
        <div className="hd">
          {status && (item.status === "failed" || item.status === "queued") && (
            <div className={`hd-error ${status.tone}`} role="alert">
              <strong>{status.label}</strong>
              <div>{status.reason}</div>
            </div>
          )}
          {status && !(item.status === "failed" || item.status === "queued") && (
            <div className="muted">{status.reason}</div>
          )}

          {shownText ? (
            <div className="hd-text">{shownText}</div>
          ) : (
            <div className="hd-text muted">There is no text for this recording.</div>
          )}

          <div className="hd-meta muted">
            {facts.join(" · ")}
            {timing.length > 0 && <div>{timing.join(" · ")}</div>}
          </div>

          {item.rawText && item.rawText !== shownText && (
            <div>
              <button className="hd-toggle" onClick={() => setShowRaw((v) => !v)} aria-expanded={showRaw}>
                {showRaw ? <ChevronDown size={14} /> : <ChevronRight size={14} />} Original transcript
              </button>
              {showRaw && <div className="hd-text hd-raw">{item.rawText}</div>}
            </div>
          )}

          {audioSrc && <audio controls src={audioSrc} className="hd-audio" />}

          <div className="hd-actions">
            <button
              className="btn btn-soft btn-sm"
              disabled={!shownText}
              onClick={() => run("copy", () => api.copyText(shownText), () => "Copied")}
            >
              <ClipboardCopy size={14} /> Copy
            </button>
            <button
              className="btn btn-soft btn-sm"
              disabled={!shownText || busy !== null}
              onClick={() =>
                run("insert", () => api.reinsertHistoryItem(item.id, false), () => "Pasted into your last app")
              }
            >
              <CornerDownLeft size={14} /> Insert again
            </button>
            {item.rawText && (
              <button
                className="btn btn-soft btn-sm"
                disabled={busy !== null}
                onClick={() =>
                  run("insertRaw", () => api.reinsertHistoryItem(item.id, true), () => "Pasted the original")
                }
              >
                Insert original
              </button>
            )}
            {canRetry && (
              <button
                className="btn btn-dark btn-sm"
                disabled={busy !== null}
                onClick={() =>
                  run(
                    "retry",
                    () => api.retryHistoryItem(item.id),
                    (p) => {
                      if (p) setShownText(p.finalText);
                      onChanged();
                      return p ? "Done. Here is the text." : "Trying again…";
                    },
                  )
                }
              >
                <RotateCcw size={14} /> {busy === "retry" ? "Trying…" : "Try again"}
              </button>
            )}
          </div>

          {item.rawText && modes.length > 0 && (
            <div className="hd-redo">
              <span className="muted">Redo as</span>
              <select className="select hd-select" value={modeId} onChange={(e) => setModeId(e.target.value)}>
                <option value="">Choose a mode…</option>
                {modes.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name}
                  </option>
                ))}
              </select>
              <button
                className="btn btn-soft btn-sm"
                disabled={!modeId || busy !== null}
                onClick={() =>
                  run(
                    "redo",
                    () => api.reprocessHistoryItem(item.id, modeId),
                    (p) => {
                      setShownText(p.finalText);
                      onChanged();
                      return `Redone as ${p.modeName}`;
                    },
                  )
                }
              >
                <Wand2 size={14} /> {busy === "redo" ? "Working…" : "Redo"}
              </button>
            </div>
          )}
        </div>
      </Modal>
      <ConfirmDialog
        open={confirmDelete}
        title="Delete this dictation?"
        message="The text and the recording will be removed for good."
        onCancel={() => setConfirmDelete(false)}
        onConfirm={async () => {
          setConfirmDelete(false);
          try {
            await api.deleteHistoryItem(item.id);
            toast("Deleted");
            onChanged();
            onClose();
          } catch (e) {
            toast(errorText(e), "error");
          }
        }}
      />
    </>
  );
}
