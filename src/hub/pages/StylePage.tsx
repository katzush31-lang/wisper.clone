import { Lightbulb, Plus, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { api, type AppStyle, type Mode, type Settings } from "../../shared/api";
import { ConfirmDialog, EmptyState, Modal, SettingRow, Switch, errorText, useToast } from "../components";
import { useSettings } from "../useSettings";
import { uniqueId } from "./library/helpers";
import "./library/library.css";

const COMMON_APPS: { key: string; name: string }[] = [
  { key: "web:gmail", name: "Gmail" },
  { key: "outlook.exe", name: "Outlook" },
  { key: "whatsapp.exe", name: "WhatsApp" },
  { key: "slack.exe", name: "Slack" },
  { key: "teams.exe", name: "Microsoft Teams" },
  { key: "claude.exe", name: "Claude" },
  { key: "web:claude", name: "Claude in browser" },
  { key: "web:chatgpt", name: "ChatGPT" },
  { key: "winword.exe", name: "Word" },
];

function prettyAppName(key: string): string {
  const base = key.replace(/^web:/, "").replace(/\.exe$/i, "");
  return base ? base.charAt(0).toUpperCase() + base.slice(1) : key;
}

export default function StylePage() {
  const { settings, update } = useSettings();
  const toast = useToast();
  const [modes, setModes] = useState<Mode[]>([]);
  const [editing, setEditing] = useState<Mode | "new" | null>(null);
  const [addingApp, setAddingApp] = useState(false);

  const sig = JSON.stringify([settings?.customModes, settings?.modeOverrides]);
  useEffect(() => {
    api.listModes().then(setModes).catch(() => setModes([]));
  }, [sig]);

  const save = async (next: Settings, okMsg?: string) => {
    try {
      await update(next);
      if (okMsg) toast(okMsg);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const setAppMode = (key: string, modeId: string) => {
    if (!settings) return;
    save({
      ...settings,
      appStyles: settings.appStyles.map((a) => (a.appKey === key ? { ...a, modeId, userSet: true } : a)),
    });
  };

  const removeApp = (key: string) => {
    if (!settings) return;
    save({ ...settings, appStyles: settings.appStyles.filter((a) => a.appKey !== key) }, "Removed");
  };

  const modeName = (id: string) => modes.find((m) => m.id === id)?.name ?? id;

  return (
    <div className="page page-narrow">
      <div className="page-header">
        <h1 className="page-title">Style</h1>
        <button className="btn btn-dark" onClick={() => setEditing("new")} disabled={!settings}>
          <Plus size={16} /> New mode
        </button>
      </div>

      <div className="lib-section-head" style={{ marginTop: 0 }}>
        <h2>Modes</h2>
      </div>
      <p className="lib-section-desc">
        A mode decides how Wisper tidies up your words — for example formal for work, or short and casual for
        friends.
      </p>
      <div className="lib-tip">
        <Lightbulb size={16} />
        <span>
          Tip: start a dictation with “as an email”, “as a list” or a mode’s name to use that mode just once.
        </span>
      </div>

      {modes.length === 0 ? (
        <EmptyState title="No modes to show">Modes will appear here once Wisper is running.</EmptyState>
      ) : (
        <div className="lib-grid">
          {modes.map((m) => {
            const isDefault = settings?.defaultMode === m.id;
            return (
              <div className={`card lib-mode-card ${isDefault ? "is-default" : ""}`} key={m.id}>
                <div className="lib-mode-head">
                  <span className="lib-mode-name">{m.name}</span>
                  {isDefault && <span className="badge lib-badge-accent">Default</span>}
                </div>
                <div className="lib-mode-desc">{m.description}</div>
                <div className="lib-mode-btns">
                  {!isDefault && settings && (
                    <button
                      className="btn btn-soft btn-sm"
                      onClick={() => save({ ...settings, defaultMode: m.id }, `${m.name} is now your default`)}
                    >
                      Make default
                    </button>
                  )}
                  <button className="btn btn-soft btn-sm" onClick={() => setEditing(m)}>
                    Edit
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}

      <div className="lib-section-head">
        <h2>Per-app styles</h2>
        <button className="btn btn-soft btn-sm" onClick={() => setAddingApp(true)} disabled={!settings}>
          <Plus size={14} /> Add app
        </button>
      </div>
      <p className="lib-section-desc">Pick which mode Wisper uses in each app, so you never have to think about it.</p>

      <div className="setting-group" style={{ marginBottom: 14 }}>
        <div className="setting-group-body">
          <SettingRow
            title="Learn styles automatically"
            description="Wisper notices which mode suits each app and remembers it. Styles you set yourself are never changed."
          >
            <Switch
              checked={!!settings?.learnAppStyles}
              disabled={!settings}
              label="Learn styles automatically"
              onChange={(v) => settings && save({ ...settings, learnAppStyles: v })}
            />
          </SettingRow>
        </div>
      </div>

      {!settings || settings.appStyles.length === 0 ? (
        <EmptyState title="No app styles yet">
          Add an app, or leave learning on and Wisper will fill this in as you dictate.
        </EmptyState>
      ) : (
        <div className="list">
          {settings.appStyles.map((a) => (
            <div className="list-row lib-row-center" key={a.appKey}>
              <div className="lib-row-main">
                <div className="lib-row-title">{a.appName || prettyAppName(a.appKey)}</div>
                <div className="lib-row-sub">{a.appKey}</div>
              </div>
              <span className={`badge ${a.userSet ? "lib-badge" : "lib-badge-accent"}`}>
                {a.userSet ? "Set by you" : "Learned"}
              </span>
              <select
                className="select lib-select"
                value={a.modeId}
                aria-label={`Mode for ${a.appName}`}
                onChange={(e) => setAppMode(a.appKey, e.target.value)}
              >
                {!modes.some((m) => m.id === a.modeId) && <option value={a.modeId}>{modeName(a.modeId)}</option>}
                {modes.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name}
                  </option>
                ))}
              </select>
              <button className="icon-btn" onClick={() => removeApp(a.appKey)} aria-label={`Remove ${a.appName}`}>
                <Trash2 size={16} />
              </button>
            </div>
          ))}
        </div>
      )}

      {editing && settings && (
        <ModeModal
          mode={editing === "new" ? null : editing}
          settings={settings}
          onClose={() => setEditing(null)}
          onSave={async (next, msg) => {
            await save(next, msg);
            setEditing(null);
          }}
        />
      )}
      {addingApp && settings && (
        <AddAppModal
          settings={settings}
          modes={modes}
          onClose={() => setAddingApp(false)}
          onSave={async (next) => {
            await save(next, "App added");
            setAddingApp(false);
          }}
        />
      )}
    </div>
  );
}

function ModeModal({
  mode,
  settings,
  onClose,
  onSave,
}: {
  mode: Mode | null;
  settings: Settings;
  onClose: () => void;
  onSave: (next: Settings, msg: string) => Promise<void>;
}) {
  const isNew = mode === null;
  const builtin = !!mode?.builtin;
  const custom = isNew || !builtin;
  const [name, setName] = useState(mode?.name ?? "");
  const [description, setDescription] = useState(mode?.description ?? "");
  const [defaultText, setDefaultText] = useState<string | null>(null);
  const override = mode ? settings.modeOverrides[mode.id] : undefined;
  const [instructions, setInstructions] = useState(override ?? mode?.instructions ?? "");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!mode || !builtin) return;
    api
      .defaultModeInstructions(mode.id)
      .then((d) => {
        setDefaultText(d);
        // if no override is saved and the list gave us nothing, show the default
        if (override === undefined && !mode.instructions && d) setInstructions(d);
      })
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode?.id]);

  const changedFromDefault = defaultText !== null && instructions !== defaultText;
  const valid = instructions.trim().length > 0 && (!custom || name.trim().length > 0);

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    try {
      await fn();
    } finally {
      setBusy(false);
    }
  };

  const save = () =>
    run(async () => {
      if (builtin && mode) {
        const overrides = { ...settings.modeOverrides };
        if (defaultText !== null && instructions === defaultText) delete overrides[mode.id];
        else overrides[mode.id] = instructions;
        await onSave({ ...settings, modeOverrides: overrides }, "Saved");
      } else if (isNew) {
        const id = uniqueId("custom_", name, [
          ...settings.customModes.map((m) => m.id),
        ]);
        const created: Mode = {
          id,
          name: name.trim(),
          description: description.trim(),
          instructions,
          builtin: false,
          aliases: [],
        };
        await onSave({ ...settings, customModes: [...settings.customModes, created] }, "Mode created");
      } else if (mode) {
        await onSave(
          {
            ...settings,
            customModes: settings.customModes.map((m) =>
              m.id === mode.id ? { ...m, name: name.trim(), description: description.trim(), instructions } : m,
            ),
          },
          "Saved",
        );
      }
    });

  const reset = () =>
    run(async () => {
      if (!mode) return;
      const overrides = { ...settings.modeOverrides };
      delete overrides[mode.id];
      await onSave({ ...settings, modeOverrides: overrides }, "Back to the original instructions");
    });

  const del = () =>
    run(async () => {
      if (!mode) return;
      await onSave(
        {
          ...settings,
          customModes: settings.customModes.filter((m) => m.id !== mode.id),
          defaultMode: settings.defaultMode === mode.id ? "auto" : settings.defaultMode,
          appStyles: settings.appStyles.map((a) => (a.modeId === mode.id ? { ...a, modeId: "auto" } : a)),
        },
        "Mode deleted",
      );
    });

  return (
    <>
      <Modal
        open
        width={600}
        title={isNew ? "New mode" : `Edit ${mode?.name}`}
        onClose={onClose}
        footer={
          <>
            {!isNew && builtin && (override !== undefined || changedFromDefault) && (
              <button className="btn btn-soft" style={{ marginRight: "auto" }} onClick={reset} disabled={busy}>
                Reset to default
              </button>
            )}
            {!isNew && !builtin && (
              <button className="btn btn-danger" style={{ marginRight: "auto" }} onClick={() => setConfirmDelete(true)}>
                Delete
              </button>
            )}
            <button className="btn btn-soft" onClick={onClose}>
              Cancel
            </button>
            <button className="btn btn-dark" onClick={save} disabled={!valid || busy}>
              Save
            </button>
          </>
        }
      >
        {custom && (
          <>
            <div className="lib-field">
              <label className="lib-label" htmlFor="mode-name">
                Name
              </label>
              <p className="lib-help">A short name. You can also say it at the start of a dictation.</p>
              <input
                id="mode-name"
                className="input"
                type="text"
                autoFocus={isNew}
                maxLength={40}
                placeholder="e.g. WhatsApp to family"
                value={name}
                onChange={(e) => setName(e.target.value)}
              />
            </div>
            <div className="lib-field">
              <label className="lib-label" htmlFor="mode-desc">
                Description
              </label>
              <p className="lib-help">One line to remind you what this mode is for.</p>
              <input
                id="mode-desc"
                className="input"
                type="text"
                maxLength={100}
                value={description}
                onChange={(e) => setDescription(e.target.value)}
              />
            </div>
          </>
        )}
        <div className="lib-field">
          <label className="lib-label" htmlFor="mode-instr">
            Instructions
          </label>
          <p className="lib-help">Plain-English instructions for how Wisper should format your words in this mode.</p>
          <textarea
            id="mode-instr"
            className="textarea lib-textarea-big"
            value={instructions}
            placeholder="WhatsApp to family: short, warm, casual, no sign-off"
            onChange={(e) => setInstructions(e.target.value)}
          />
        </div>
      </Modal>
      <ConfirmDialog
        open={confirmDelete}
        title="Delete this mode?"
        message={`“${mode?.name}” will be removed. Anything using it goes back to Auto.`}
        onConfirm={() => {
          setConfirmDelete(false);
          del();
        }}
        onCancel={() => setConfirmDelete(false)}
      />
    </>
  );
}

function AddAppModal({
  settings,
  modes,
  onClose,
  onSave,
}: {
  settings: Settings;
  modes: Mode[];
  onClose: () => void;
  onSave: (next: Settings) => Promise<void>;
}) {
  const [choice, setChoice] = useState(COMMON_APPS[0].key);
  const [other, setOther] = useState("");
  const [modeId, setModeId] = useState(modes[0]?.id ?? "auto");
  const key = (choice === "other" ? other : choice).trim().toLowerCase();

  const add = () => {
    if (!key) return;
    const name = choice === "other" ? prettyAppName(key) : (COMMON_APPS.find((c) => c.key === choice)?.name ?? key);
    const style: AppStyle = { appKey: key, appName: name, modeId, userSet: true };
    const rest = settings.appStyles.filter((a) => a.appKey !== key);
    onSave({ ...settings, appStyles: [...rest, style] });
  };

  return (
    <Modal
      open
      title="Add an app"
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-soft" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-dark" onClick={add} disabled={!key}>
            Add
          </button>
        </>
      }
    >
      <div className="lib-field">
        <label className="lib-label" htmlFor="app-pick">
          App or website
        </label>
        <p className="lib-help">Pick one from the list, or choose “Other…” to type its name.</p>
        <select id="app-pick" className="select" value={choice} onChange={(e) => setChoice(e.target.value)}>
          {COMMON_APPS.map((a) => (
            <option key={a.key} value={a.key}>
              {a.name} ({a.key})
            </option>
          ))}
          <option value="other">Other…</option>
        </select>
        {choice === "other" && (
          <>
            <p className="lib-help" style={{ marginTop: 10 }}>
              Type the program’s file name, like “whatsapp.exe”, or a website like “web:gmail”.
            </p>
            <input
              className="input"
              type="text"
              autoFocus
              placeholder="whatsapp.exe or web:gmail"
              value={other}
              onChange={(e) => setOther(e.target.value)}
            />
          </>
        )}
      </div>
      <div className="lib-field">
        <label className="lib-label" htmlFor="app-mode">
          Mode
        </label>
        <p className="lib-help">How Wisper should write in this app.</p>
        <select id="app-mode" className="select" value={modeId} onChange={(e) => setModeId(e.target.value)}>
          {modes.map((m) => (
            <option key={m.id} value={m.id}>
              {m.name}
            </option>
          ))}
        </select>
      </div>
    </Modal>
  );
}
