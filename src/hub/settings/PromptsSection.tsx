import { Copy } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api, type AppCategory, type CleanupLevel, type Mode, type PromptPreview } from "../../shared/api";
import { SettingGroup, SettingRow, errorText, useToast } from "../components";
import type { SectionProps } from "./GeneralSection";
import { commandPromptText, dictationPromptText } from "./promptsText";

/** A read-only block of exactly what is sent, in monospace, scrollable, with a Copy button. */
function PromptBlock({ title, description, text, tall }: { title: string; description: string; text: string; tall?: boolean }) {
  const toast = useToast();
  return (
    <div className="prompt-block">
      <div className="prompt-block-head">
        <div>
          <div className="prompt-block-title">{title}</div>
          <div className="muted prompt-block-desc">{description}</div>
        </div>
        <button
          className="btn btn-soft btn-sm"
          aria-label={`Copy: ${title}`}
          onClick={() =>
            api
              .copyText(text)
              .then(() => toast("Copied"))
              .catch((e) => toast(errorText(e), "error"))
          }
        >
          <Copy size={14} /> Copy
        </button>
      </div>
      <pre className={`prompt-pre ${tall ? "tall" : ""}`} tabIndex={0} aria-label={title}>
        {text}
      </pre>
    </div>
  );
}

const LEVELS: { id: CleanupLevel; label: string }[] = [
  { id: "none", label: "No AI edits" },
  { id: "light", label: "Light" },
  { id: "medium", label: "Medium" },
];

export default function PromptsSection({ settings, onNavigate }: SectionProps & { onNavigate?: (page: "style") => void }) {
  const toast = useToast();
  const [modes, setModes] = useState<Mode[]>([]);
  const [modeId, setModeId] = useState(settings.defaultMode || "auto");
  const [category, setCategory] = useState<AppCategory | "">("");
  const [level, setLevel] = useState<CleanupLevel>(settings.cleanupLevel);
  const [preview, setPreview] = useState<PromptPreview | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const seq = useRef(0);

  useEffect(() => {
    api
      .listModes()
      .then((m) => {
        setModes(m);
        setModeId((cur) => (m.some((x) => x.id === cur) ? cur : "auto"));
      })
      .catch((e) => toast(errorText(e), "error"));
  }, [toast]);

  useEffect(() => {
    const mine = ++seq.current;
    api
      .promptPreview(modeId, category || null, level)
      .then((p) => {
        if (mine !== seq.current) return;
        setPreview(p);
        setFailed(null);
      })
      .catch((e) => {
        if (mine === seq.current) setFailed(errorText(e));
      });
  }, [modeId, category, level]);

  return (
    <>
      <p className="muted prompt-intro">
        This is exactly what Wisper sends to the AI that tidies up your dictation. It is read only. To change how a style
        writes, edit it on the Style page.{" "}
        {onNavigate && (
          <button className="link-btn" onClick={() => onNavigate("style")}>
            Open the Style page
          </button>
        )}
      </p>

      <SettingGroup title="Pick what to look at">
        <SettingRow title="Style" description="Which writing style's instructions to show. Your own edits and custom styles are included.">
          <select className="select settings-select" aria-label="Style" value={modeId} onChange={(e) => setModeId(e.target.value)}>
            {modes.map((m) => (
              <option key={m.id} value={m.id}>
                {m.name}
              </option>
            ))}
          </select>
        </SettingRow>
        <SettingRow title="App type" description="The kind of app you are typing into. It changes the note the AI gets about where the text is going.">
          <select
            className="select settings-select"
            aria-label="App type"
            value={category}
            onChange={(e) => setCategory(e.target.value as AppCategory | "")}
          >
            <option value="">No particular app</option>
            {(preview?.appTypes ?? []).map((a) => (
              <option key={a.category} value={a.category}>
                {a.label}
              </option>
            ))}
          </select>
        </SettingRow>
        <SettingRow title="Cleanup level" description="How much the AI is allowed to change what you said. Set in Settings under Dictation.">
          <select className="select settings-select" aria-label="Cleanup level" value={level} onChange={(e) => setLevel(e.target.value as CleanupLevel)}>
            {LEVELS.map((l) => (
              <option key={l.id} value={l.id}>
                {l.label}
              </option>
            ))}
          </select>
        </SettingRow>
      </SettingGroup>

      {failed && <p className="muted" role="alert">{failed}</p>}

      {preview && (
        <>
          {!preview.aiUsed && (
            <p className="prompt-note" role="note">
              With "No AI edits" nothing is sent to the AI at all: only your dictionary spellings, snippets and spoken line
              breaks are applied. The text below is what would be sent at the Light or Medium level.
            </p>
          )}

          <SettingGroup title="What is sent for a dictation">
            <div className="prompt-stack">
              <PromptBlock
                title="1. Shared instructions and your dictionary"
                description="Sent with every dictation, whatever the style. The dictionary at the bottom is your real one, listed the way the AI sees it."
                text={preview.baseSystem}
                tall
              />
              <PromptBlock
                title={`2. The ${preview.modeName} style, and how much to edit`}
                description="The style's instructions (with your edits) followed by the cleanup level wording."
                text={preview.modeSystem}
              />
              <PromptBlock
                title="3. The message for this dictation (example)"
                description="Where the text is going, text before your cursor if that setting is on, then what you said. The sample sentence here is made up."
                text={preview.userMessage}
              />
              <button
                className="btn btn-soft btn-sm prompt-copy-all"
                onClick={() =>
                  api
                    .copyText(dictationPromptText(preview))
                    .then(() => toast("Copied"))
                    .catch((e) => toast(errorText(e), "error"))
                }
              >
                <Copy size={14} /> Copy all three
              </button>
            </div>
          </SettingGroup>

          <SettingGroup title="How the kind of app changes things">
            <p className="muted prompt-intro">
              Two things depend on the app. The AI is told which kind of app the text is going into (the sentence below), and
              in Auto style Wisper leans towards a matching style unless you chose one for that app.
            </p>
            <div className="prompt-table" role="table" aria-label="App types">
              {preview.appTypes.map((a) => (
                <div className="prompt-table-row" role="row" key={a.category}>
                  <div className="prompt-table-label" role="cell">{a.label}</div>
                  <div className="prompt-table-text" role="cell">
                    <code>{a.sentence}</code>
                    <div className="muted">Auto style picks: {a.autoModeName}</div>
                  </div>
                </div>
              ))}
            </div>
          </SettingGroup>

          <SettingGroup title="Cleanup level wording">
            <PromptBlock
              title={LEVELS.find((l) => l.id === preview.level)?.label ?? "Level"}
              description="The last paragraph of block 2. None and Light use the same wording because None sends nothing."
              text={preview.levelText}
            />
          </SettingGroup>

          <SettingGroup title="Voice commands (select text, then speak an instruction)">
            <div className="prompt-stack">
              <PromptBlock
                title="Instructions for the AI"
                description="Used when you select text and say what to do with it. Only words you added or used are listed from your dictionary."
                text={preview.commandSystem}
              />
              <PromptBlock
                title="The message (example)"
                description="The selected text and your spoken instruction. The sample is made up."
                text={preview.commandUser}
              />
              <button
                className="btn btn-soft btn-sm prompt-copy-all"
                onClick={() =>
                  api
                    .copyText(commandPromptText(preview))
                    .then(() => toast("Copied"))
                    .catch((e) => toast(errorText(e), "error"))
                }
              >
                <Copy size={14} /> Copy both
              </button>
            </div>
          </SettingGroup>

          <SettingGroup title="Transforms">
            <p className="muted prompt-intro">
              A transform sends the same voice-command instructions as above, with the transform's own text as the instruction.
              Edit them on the Transforms page.
            </p>
            <div className="prompt-stack">
              {preview.transforms.map((t) => (
                <PromptBlock key={t.name} title={t.name} description="The message sent for this transform, with a made-up selection." text={t.userMessage} />
              ))}
            </div>
          </SettingGroup>

          <SettingGroup title="Every style's instructions">
            <PromptBlock
              title="All styles"
              description="Your current wording for each style, including edits and custom styles."
              text={modes.map((m) => `## ${m.name}\n${m.instructions.trim()}`).join("\n\n")}
              tall
            />
          </SettingGroup>
        </>
      )}
    </>
  );
}
