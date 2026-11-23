import { useEffect, useRef, useState } from "react";
import { api, comboKeyNames, events, type HotkeyConfig, type KeyCombo } from "../../shared/api";
import { ConfirmDialog, Modal, SettingGroup, SettingRow, Switch, errorText, useToast } from "../components";
import { DEFAULT_HOTKEYS, HOTKEY_ACTIONS, doubleClickChoice, mouseButtonToVk, setCombo } from "./helpers";
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
      {comboKeyNames(combo).map((k, i) => (
          <kbd key={i} className="keycap">
            {k}
          </kbd>
        ))}
    </span>
  );
}

const MODIFIER_VKS = [0x10, 0x11, 0x12, 0x5b, 0x5c];

function CaptureDialog({
  title,
  onSave,
  onClose,
}: {
  title: string;
  onSave: (keys: number[], doubleClick: boolean) => void;
  onClose: () => void;
}) {
  const [keys, setKeys] = useState<number[]>([]);
  const [doubleClick, setDoubleClick] = useState(false);
  const [label, setLabel] = useState("");
  const [done, setDone] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  // Keys and buttons pressed in this window, in case Windows' hooks don't report them.
  const local = useRef<{ keys: number[]; held: number[]; done: boolean }>({ keys: [], held: [], done: false });
  const choice = done ? doubleClickChoice(keys) : "none";

  const describe = (k: number[], dbl: boolean) => {
    api
      .describeCombo(k, dbl)
      .then((d) => {
        setLabel(d.label);
        setError(d.error);
      })
      .catch((e) => setError(errorText(e)));
  };

  const show = (k: number[], isDone: boolean) => {
    setKeys(k);
    setError(null);
    setDone(isDone);
    if (isDone) {
      // A mouse button on its own starts as a double-click, so single clicks keep working.
      const dbl = doubleClickChoice(k) !== "none";
      setDoubleClick(dbl);
      describe(k, dbl);
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

    const press = (vk: number) => {
      const l = local.current;
      if (!l.held.includes(vk)) l.held.push(vk);
      if (!l.keys.includes(vk) && l.keys.length < 4) l.keys.push(vk);
      show([...l.keys], false);
    };
    const release = (vk: number): boolean => {
      const l = local.current;
      if (l.done || !l.held.includes(vk)) return false;
      l.held = l.held.filter((k) => k !== vk);
      // Windows often swallows the Win key's release, so finish when the last non-Win key is up.
      if (l.held.filter((k) => k !== 91 && k !== 92).length === 0 && l.keys.length > 0) {
        l.done = true;
        l.held = [];
        show([...l.keys], true);
      }
      return true;
    };
    const onDown = (e: KeyboardEvent) => {
      const vk = keyToVk(e);
      if (vk === null || e.key === "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      if (local.current.done) return;
      press(vk);
    };
    const onUp = (e: KeyboardEvent) => {
      const vk = keyToVk(e);
      if (vk !== null && release(vk)) e.preventDefault();
    };
    // Mouse buttons: the wheel click and side buttons always count; left/right only with a
    // modifier held, so this dialog's own buttons can still be clicked.
    const mouseVk = (e: MouseEvent): number | null => {
      const vk = mouseButtonToVk(e.button);
      if (vk === null) return null;
      if ((vk === 0x01 || vk === 0x02) && !local.current.held.some((k) => MODIFIER_VKS.includes(k))) return null;
      return vk;
    };
    const onMouseDown = (e: MouseEvent) => {
      const vk = mouseVk(e);
      if (vk === null || local.current.done) return;
      e.preventDefault();
      e.stopPropagation();
      press(vk);
    };
    const onMouseUp = (e: MouseEvent) => {
      const vk = mouseButtonToVk(e.button);
      if (vk !== null && release(vk)) {
        e.preventDefault();
        e.stopPropagation();
      }
    };
    // Stop the browser's own middle-click and back/forward actions while recording.
    const onAux = (e: MouseEvent) => {
      if (e.button === 1 || e.button === 3 || e.button === 4) e.preventDefault();
    };
    window.addEventListener("keydown", onDown, true);
    window.addEventListener("keyup", onUp, true);
    window.addEventListener("mousedown", onMouseDown, true);
    window.addEventListener("mouseup", onMouseUp, true);
    window.addEventListener("auxclick", onAux, true);
    return () => {
      dead = true;
      off?.();
      window.removeEventListener("keydown", onDown, true);
      window.removeEventListener("keyup", onUp, true);
      window.removeEventListener("mousedown", onMouseDown, true);
      window.removeEventListener("mouseup", onMouseUp, true);
      window.removeEventListener("auxclick", onAux, true);
      api.stopHotkeyCapture().catch(() => {});
    };
  }, []);

  const retry = () => {
    local.current = { keys: [], held: [], done: false };
    setKeys([]);
    setDoubleClick(false);
    setLabel("");
    setDone(false);
    setError(null);
    api.startHotkeyCapture().catch((e) => setProblem(errorText(e)));
  };

  const changeDoubleClick = (v: boolean) => {
    setDoubleClick(v);
    describe(keys, v);
  };

  const canSave = done && !error && keys.length > 0;
  const shown: KeyCombo = { keys, doubleClick: done && doubleClick };

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
          <button className="btn btn-dark" disabled={!canSave} onClick={() => onSave(keys, choice !== "none" && doubleClick)}>
            Save
          </button>
        </>
      }
    >
      <div className="capture-box" aria-live="polite">
        {keys.length === 0 ? (
          <span className="muted">Press and hold the keys you want (or click a mouse button), then let go…</span>
        ) : (
          <KeyCaps combo={shown} />
        )}
      </div>
      {choice === "optional" && (
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12, marginTop: 12 }}>
          <span>
            <strong>Double-click</strong>
            <br />
            <span className="muted">
              {doubleClick
                ? "Double-click to start, click once to finish. A single click still goes Back/Forward in apps."
                : "Works like a key: hold to talk, press twice for hands-free. The button stops going Back/Forward in apps."}
            </span>
          </span>
          <Switch label="Double-click" checked={doubleClick} onChange={changeDoubleClick} />
        </div>
      )}
      {choice === "forced" && (
        <p className="muted capture-note">
          Double-click the scroll wheel to start, click it once to finish. Single clicks keep working in your apps (the first click of the
          double-click still reaches the app, so do it on an empty spot).
        </p>
      )}
      {label && done && !error && <p className="muted capture-note">New shortcut: {label}</p>}
      {error && <p className="setting-warn capture-note">{error}</p>}
      {problem && <p className="setting-warn capture-note">{problem}</p>}
      <p className="muted capture-note">
        Tip: use two or more keys together, such as Ctrl + Win, so it never triggers by accident. Mouse buttons work too: the scroll-wheel
        click, the side buttons, or Ctrl/Alt/Shift/Win with any button.
      </p>
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
        Hold to talk, release to paste. Double-tap to keep recording hands-free; tap again to finish. Esc cancels. Mouse shortcuts: double-click
        to start, click once to finish. Shortcuts work wherever you are; if no text box is selected, the text is kept in History with a Copy
        button.
      </p>
      <SettingGroup>
        <SettingRow
          title="Shortcuts enabled"
          description="Turn off to make Wisper ignore all its keyboard shortcuts until you turn them back on. You can also do this from the tray icon."
        >
          <Switch label="Shortcuts enabled" checked={settings.shortcutsEnabled} onChange={(v) => save({ ...settings, shortcutsEnabled: v })} />
        </SettingRow>
      </SettingGroup>
      {!settings.shortcutsEnabled && <p className="muted section-note">Shortcuts are switched off, so pressing them does nothing. You can still change them below.</p>}
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
          onSave={(keys, doubleClick) => {
            const id = editing;
            setEditing(null);
            void save({ ...settings, hotkeys: setCombo(settings.hotkeys, id, keys, doubleClick) }).then(() => toast("Shortcut saved"));
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
