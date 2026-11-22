import { useEffect, useRef, useState } from "react";
import { api, comboLabel, events, type HotkeyConfig, type KeyCombo } from "../../shared/api";
import { ConfirmDialog, Modal, SettingGroup, SettingRow, errorText, useToast } from "../components";
import { DEFAULT_HOTKEYS, HOTKEY_ACTIONS, setCombo } from "./helpers";
import { useSaver, type SectionProps } from "./GeneralSection";

function KeyCaps({ combo }: { combo: KeyCombo | undefined }) {
  if (!combo || combo.keys.length === 0) return <span className="muted">Not set</span>;
  return (
    <span className="keycaps">
      {comboLabel(combo)
        .split(" + ")
        .map((k, i) => (
          <kbd key={i} className="keycap">
            {k}
          </kbd>
        ))}
    </span>
  );
}

function CaptureDialog({
  title,
  onSave,
  onClose,
}: {
  title: string;
  onSave: (keys: number[]) => void;
  onClose: () => void;
}) {
  const [keys, setKeys] = useState<number[]>([]);
  const [label, setLabel] = useState("");
  const [done, setDone] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const active = useRef(true);

  useEffect(() => {
    active.current = true;
    let off: (() => void) | undefined;
    let dead = false;
    events
      .hotkeyCaptured((r) => {
        setKeys(r.keys);
        setLabel(r.label);
        setError(null);
        if (r.done) {
          setDone(true);
          api
            .describeCombo(r.keys)
            .then((d) => {
              setLabel(d.label);
              setError(d.error);
            })
            .catch((e) => setError(errorText(e)));
        } else {
          setDone(false);
        }
      })
      .then((u) => (dead ? u() : (off = u)));
    api.startHotkeyCapture().catch((e) => setProblem(errorText(e)));
    return () => {
      dead = true;
      off?.();
      api.stopHotkeyCapture().catch(() => {});
    };
  }, []);

  const retry = () => {
    setKeys([]);
    setLabel("");
    setDone(false);
    setError(null);
    api.startHotkeyCapture().catch((e) => setProblem(errorText(e)));
  };

  const canSave = done && !error && keys.length > 0;

  return (
    <Modal
      open
      title={`Change shortcut: ${title}`}
      onClose={onClose}
      width={460}
      footer={
        <>
          {done && (
            <button className="btn btn-soft" onClick={retry}>
              Try again
            </button>
          )}
          <button className="btn btn-soft" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-dark" disabled={!canSave} onClick={() => onSave(keys)}>
            Save
          </button>
        </>
      }
    >
      <div className="capture-box" aria-live="polite">
        {keys.length === 0 ? (
          <span className="muted">Press and hold the keys you want, then let go…</span>
        ) : (
          <KeyCaps combo={{ keys }} />
        )}
      </div>
      {label && done && !error && <p className="muted capture-note">New shortcut: {label}</p>}
      {error && <p className="setting-warn capture-note">{error}</p>}
      {problem && <p className="setting-warn capture-note">{problem}</p>}
      <p className="muted capture-note">Tip: use two or more keys together, such as Ctrl + Win, so it never triggers by accident.</p>
    </Modal>
  );
}

export default function ShortcutsSection({ settings, update, onBusy }: SectionProps & { onBusy: (busy: boolean) => void }) {
  const save = useSaver(update);
  const toast = useToast();
  const [editing, setEditing] = useState<keyof HotkeyConfig | null>(null);
  const [confirmReset, setConfirmReset] = useState(false);
  const editingTitle = HOTKEY_ACTIONS.find((a) => a.id === editing)?.title ?? "";

  useEffect(() => {
    onBusy(editing !== null || confirmReset);
    return () => onBusy(false);
  }, [editing, confirmReset, onBusy]);

  return (
    <>
      <p className="section-intro">
        Hold to talk, release to paste. Double-tap to keep recording hands-free; tap again to finish. Esc cancels.
      </p>
      <SettingGroup>
        {HOTKEY_ACTIONS.map((a) => (
          <SettingRow key={a.id} title={a.title} description={a.description}>
            <div className="combo-list">
              {settings.hotkeys[a.id].length === 0 ? <KeyCaps combo={undefined} /> : settings.hotkeys[a.id].map((c, i) => <KeyCaps key={i} combo={c} />)}
            </div>
            <button className="btn btn-soft" onClick={() => setEditing(a.id)} aria-label={`Change shortcut for ${a.title}`}>
              Change
            </button>
          </SettingRow>
        ))}
        <SettingRow title="Reset all to defaults" description="Puts every shortcut back to how it came (the same ones Wispr Flow uses).">
          <button className="btn btn-soft" onClick={() => setConfirmReset(true)}>
            Reset all
          </button>
        </SettingRow>
      </SettingGroup>

      {editing && (
        <CaptureDialog
          title={editingTitle}
          onClose={() => setEditing(null)}
          onSave={(keys) => {
            const id = editing;
            setEditing(null);
            void save({ ...settings, hotkeys: setCombo(settings.hotkeys, id, keys) }).then(() => toast("Shortcut saved"));
          }}
        />
      )}
      <ConfirmDialog
        open={confirmReset}
        title="Reset shortcuts?"
        message="All shortcuts go back to their defaults. Your other settings are not touched."
        confirmLabel="Reset"
        onCancel={() => setConfirmReset(false)}
        onConfirm={() => {
          setConfirmReset(false);
          void save({ ...settings, hotkeys: DEFAULT_HOTKEYS }).then(() => toast("Shortcuts reset"));
        }}
      />
    </>
  );
}
