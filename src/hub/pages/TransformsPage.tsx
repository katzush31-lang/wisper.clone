import { Pencil, Play, Plus, Trash2, Wand2 } from "lucide-react";
import { useState } from "react";
import { api, comboLabel, type Settings, type Transform } from "../../shared/api";
import { ConfirmDialog, EmptyState, Modal, errorText, useToast } from "../components";
import { useSettings } from "../useSettings";
import { truncate, uniqueId } from "./library/helpers";
import "./library/library.css";

export default function TransformsPage() {
  const { settings, update } = useSettings();
  const toast = useToast();
  const [editing, setEditing] = useState<Transform | "new" | null>(null);
  const [deleting, setDeleting] = useState<Transform | null>(null);
  const list = settings?.transforms ?? [];

  const save = async (next: Settings, msg: string) => {
    try {
      await update(next);
      toast(msg);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const tryIt = async (t: Transform) => {
    try {
      await api.runTransform(t.id);
      toast(`Running “${t.name}” on your selected text…`);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <div className="page page-narrow">
      <div className="page-header">
        <h1 className="page-title">Transforms</h1>
        <button className="btn btn-dark" onClick={() => setEditing("new")} disabled={!settings}>
          <Plus size={16} /> New transform
        </button>
      </div>

      <div className="card lib-banner-plain">
        <Wand2 size={20} />
        <p>
          Select text in any app and press a shortcut to rewrite it. “Try it” works on whatever text is selected in
          the last app you used. Shortcuts can be changed in Settings → Shortcuts for now.
        </p>
      </div>

      {list.length === 0 ? (
        <EmptyState icon={<Wand2 size={28} />} title="No transforms yet">
          Add one, like “Make it shorter”, and Wisper will rewrite any text you select.
        </EmptyState>
      ) : (
        <div className="list">
          {list.map((t) => (
            <div className="list-row lib-row-center" key={t.id}>
              <div className="lib-row-main">
                <div className="lib-row-title">
                  {t.name}
                  <span className="badge lib-badge">{t.hotkey ? comboLabel(t.hotkey) : "No shortcut"}</span>
                </div>
                <div className="lib-row-sub">{truncate(t.instructions, 120)}</div>
              </div>
              <div className="lib-row-actions">
                <button className="btn btn-soft btn-sm" onClick={() => tryIt(t)} title="Rewrites the text currently selected in the last app you used">
                  <Play size={13} /> Try it
                </button>
                <button className="icon-btn" onClick={() => setEditing(t)} aria-label={`Edit ${t.name}`}>
                  <Pencil size={16} />
                </button>
                {!t.builtin && (
                  <button className="icon-btn" onClick={() => setDeleting(t)} aria-label={`Delete ${t.name}`}>
                    <Trash2 size={16} />
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>
      )}

      {editing && settings && (
        <TransformModal
          transform={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSave={async (name, instructions) => {
            if (editing === "new") {
              const t: Transform = {
                id: uniqueId("tf_", name, settings.transforms.map((x) => x.id)),
                name,
                instructions,
                hotkey: null,
                builtin: false,
              };
              await save({ ...settings, transforms: [...settings.transforms, t] }, "Transform added");
            } else {
              await save(
                {
                  ...settings,
                  transforms: settings.transforms.map((x) => (x.id === editing.id ? { ...x, name, instructions } : x)),
                },
                "Saved",
              );
            }
            setEditing(null);
          }}
        />
      )}
      <ConfirmDialog
        open={!!deleting}
        title="Delete this transform?"
        message={`“${deleting?.name}” will be removed.`}
        onConfirm={async () => {
          if (settings && deleting) {
            await save({ ...settings, transforms: settings.transforms.filter((x) => x.id !== deleting.id) }, "Deleted");
          }
          setDeleting(null);
        }}
        onCancel={() => setDeleting(null)}
      />
    </div>
  );
}

function TransformModal({
  transform,
  onClose,
  onSave,
}: {
  transform: Transform | null;
  onClose: () => void;
  onSave: (name: string, instructions: string) => Promise<void>;
}) {
  const [name, setName] = useState(transform?.name ?? "");
  const [instructions, setInstructions] = useState(transform?.instructions ?? "");
  const [busy, setBusy] = useState(false);
  const valid = name.trim().length > 0 && instructions.trim().length > 0;
  return (
    <Modal
      open
      title={transform ? "Edit transform" : "New transform"}
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-soft" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn btn-dark"
            disabled={!valid || busy}
            onClick={async () => {
              setBusy(true);
              await onSave(name.trim(), instructions);
              setBusy(false);
            }}
          >
            Save
          </button>
        </>
      }
    >
      <div className="lib-field">
        <label className="lib-label" htmlFor="tf-name">
          Name
        </label>
        <p className="lib-help">A short name so you can recognise it.</p>
        <input
          id="tf-name"
          className="input"
          type="text"
          autoFocus
          maxLength={40}
          placeholder="e.g. Make it shorter"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
      </div>
      <div className="lib-field">
        <label className="lib-label" htmlFor="tf-instr">
          Instructions
        </label>
        <p className="lib-help">Tell Wisper in plain English how to rewrite the text you select.</p>
        <textarea
          id="tf-instr"
          className="textarea"
          rows={6}
          placeholder="Rewrite this to be half as long. Keep my meaning and my tone."
          value={instructions}
          onChange={(e) => setInstructions(e.target.value)}
        />
      </div>
    </Modal>
  );
}
