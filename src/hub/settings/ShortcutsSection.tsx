import { useEffect, useRef, useState } from "react";
import { api, comboLabel, events, type HotkeyConfig, type KeyCombo } from "../../shared/api";
import { ConfirmDialog, Modal, SettingGroup, SettingRow, errorText, useToast } from "../components";
import { DEFAULT_HOTKEYS, HOTKEY_ACTIONS, setCombo } from "./helpers";
import { useSaver, type SectionProps } from "./GeneralSection";

/** Windows virtual-key code for a browser key event (browsers report Windows codes in keyCode). */
export function keyToVk(e: { key: string; code: string; keyCode: number }): number | null {
  if (e.key === "Control") return 17;
  if (e.key === "Shift") return 16;
  if (e.key === "Alt" || e.key === "AltGraph") return 18;
  if (e.key === "Meta" || e.key === "OS") return 91;
  if (e.code === "Space") return 32;
  if (/^Key[A-Z]$/.test(e.code)) return e.code.charCodeAt(3);
  if (/^Digit[0-9]$/.test(e.code)) return e.code.charCodeAt(5);
  const f = /^F([0-9]{1,2})$/.exec(e.code);
  if (f) return 111 + Number(f[1]);
  return e.keyCode > 0 && e.keyCode < 256 ? e.keyCode : null;
}

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
  // Keys pressed in this window, in case Windows' keyboard hook doesn't report them.
  const local = useRef<{ keys: number[]; held: number[]; done: boolean }>({ keys: [], held: [], done: false });

  const show = (k: number[], isDone: boolean) => {
    setKeys(k);
    setError(null);
    setDone(isDone);
    if (isDone) {
      api
        .describeCombo(k)
        .then((d) => {
          setLabel(d.label);
          setError(d.error);
        })
        .catch((e) => setError(errorText(e)));
    }
  };

  useEffect(() => {
    let off: (() => void) | undefined;
    let dead = false;
    events
      .hotkeyCaptured((r) => {
        local.current.done = r.done;
        setLabel(r.label);
        show(r.keys, r.done);
      })
      .then((u) => (dead ? u() : (off = u)));
    api.startHotkeyCapture().catch((e) => setProblem(errorText(e)));

    const onDown = (e: KeyboardEvent) => {
      const vk = keyToVk(e);
      if (vk === null || e.key === "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      const l = local.current;
      if (l.done) return;
      if (!l.held.includes(vk)) l.held.push(vk);
      if (!l.keys.includes(vk) && l.keys.length < 4) l.keys.push(vk);
      show([...l.keys], false);
    };
    const onUp = (e: KeyboardEvent) => {
      const vk = keyToVk(e);
      const l = local.current;
      if (vk === null || l.done || !l.held.includes(vk)) return;
      e.preventDefault();
      l.held = l.held.filter((k) => k !== vk);
      // Windows often swallows the Win key's release, so finish when the last non-Win key is up.
      if (l.held.filter((k) => k !== 91 && k !== 92).length === 0 && l.keys.length > 0) {
        l.done = true;
        l.held = [];
        show([...l.keys], true);
      }
    };
    window.addEventListener("keydown", onDown, true);
    window.addEventListener("keyup", onUp, true);
    return () => {
      dead = true;
      off?.();
      window.removeEventListener("keydown", onDown, true);
      window.removeEventListener("keyup", onUp, true);
      api.stopHotkeyCapture().catch(() => {});
    };
  }, []);

  const retry = () => {
    local.current = { keys: [], held: [], done: false };
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
            <button type="button" className="combo-list combo-click" onClick={() => setEditing(a.id)} title="Click to change">
              {settings.hotkeys[a.id].length === 0 ? <KeyCaps combo={undefined} /> : settings.hotkeys[a.id].map((c, i) => <KeyCaps key={i} combo={c} />)}
            </button>
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
