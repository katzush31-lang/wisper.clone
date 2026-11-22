import { Check, ExternalLink, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { api, type CleanupProvider, type ModelInfo, type Provider, type Settings } from "../../shared/api";
import { openUrl } from "../../shared/tauri";
import { Segmented, SettingGroup, SettingRow, errorText, useToast } from "../components";
import { useSaver, type SectionProps } from "./GeneralSection";

function ModelSelect({
  models,
  value,
  onChange,
  label,
}: {
  models: ModelInfo[] | null;
  value: string;
  onChange: (id: string) => void;
  label: string;
}) {
  const current = models?.find((m) => m.id === value);
  return (
    <>
      <select className="select settings-select" value={value} onChange={(e) => onChange(e.target.value)} aria-label={label}>
        {!current && <option value={value}>{value}</option>}
        {(models ?? []).map((m) => (
          <option key={m.id} value={m.id}>
            {m.name}
            {m.recommended ? " (recommended)" : ""}
          </option>
        ))}
      </select>
      {current?.note && <div className="model-note">{current.note}</div>}
    </>
  );
}

function KeyBlock({
  provider,
  title,
  description,
  url,
  saved,
  onSaved,
}: {
  provider: Provider;
  title: string;
  description: string;
  url: string;
  saved: boolean;
  onSaved: () => void;
}) {
  const toast = useToast();
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; message: string } | null>(null);

  const doSave = async () => {
    if (!key.trim()) return;
    setBusy(true);
    try {
      await api.setApiKey(provider, key.trim());
      setKey("");
      setResult(null);
      onSaved();
      toast("Key saved");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const doTest = async () => {
    setBusy(true);
    setResult(null);
    try {
      setResult(await api.testApiKey(provider, key.trim() || undefined));
    } catch (e) {
      setResult({ ok: false, message: errorText(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingRow
      title={title}
      description={
        <>
          {description}{" "}
          <button className="link-btn" onClick={() => void openUrl(url)}>
            Get a key <ExternalLink size={12} />
          </button>
        </>
      }
      stacked
    >
      <div className="key-row">
        <input
          className="input"
          type="password"
          autoComplete="off"
          value={key}
          placeholder={saved ? "Paste a new key to replace the saved one" : "Paste your key here"}
          onChange={(e) => setKey(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && void doSave()}
          aria-label={title}
        />
        <button className="btn btn-dark" disabled={busy || !key.trim()} onClick={doSave}>
          Save
        </button>
        <button className="btn btn-soft" disabled={busy || (!saved && !key.trim())} onClick={doTest}>
          Test
        </button>
      </div>
      <div className="key-status">
        {saved ? <span className="badge badge-ok">A key is saved</span> : <span className="badge badge-none">No key yet</span>}
        {result && (
          <span className={result.ok ? "test-ok" : "test-bad"} role="status">
            {result.ok ? <Check size={14} /> : <X size={14} />} {result.message}
          </span>
        )}
      </div>
    </SettingRow>
  );
}

export default function ProvidersSection({ settings, update }: SectionProps) {
  const save = useSaver(update);
  const [status, setStatus] = useState<{ openai: boolean; anthropic: boolean }>({ openai: false, anthropic: false });
  const [speech, setSpeech] = useState<ModelInfo[] | null>(null);
  const [claude, setClaude] = useState<ModelInfo[] | null>(null);
  const [openai, setOpenai] = useState<ModelInfo[] | null>(null);

  const refresh = useCallback(() => {
    api.keyStatus().then(setStatus).catch(() => {});
  }, []);
  useEffect(() => {
    refresh();
    api.listModels("transcription").then(setSpeech).catch(() => setSpeech([]));
    api.listModels("anthropic").then(setClaude).catch(() => setClaude([]));
    api.listModels("openai").then(setOpenai).catch(() => setOpenai([]));
  }, [refresh]);

  const setProviders = (patch: Partial<Settings["providers"]>) => save({ ...settings, providers: { ...settings.providers, ...patch } });

  return (
    <>
      <SettingGroup title="OpenAI API key (used for speech-to-text)">
        <KeyBlock
          provider="openai"
          title="OpenAI API key"
          description="Turns your voice into text. Your recording is sent to OpenAI for this."
          url="https://platform.openai.com/api-keys"
          saved={status.openai}
          onSaved={refresh}
        />
        <SettingRow title="Speech-to-text model" description="The model that listens to you. The recommended one is the best balance of accuracy and price." stacked>
          <ModelSelect label="Speech-to-text model" models={speech} value={settings.providers.transcriptionModel} onChange={(id) => setProviders({ transcriptionModel: id })} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="Claude API key (used for cleanup)">
        <KeyBlock
          provider="anthropic"
          title="Claude API key"
          description="Tidies up your text: punctuation, ums and wording. Only the words are sent, never the audio."
          url="https://platform.claude.com/settings/keys"
          saved={status.anthropic}
          onSaved={refresh}
        />
        <SettingRow title="Claude model" description="The Claude model that does the cleanup. The recommended one is the best balance of quality and speed." stacked>
          <ModelSelect label="Claude model" models={claude} value={settings.providers.anthropicModel} onChange={(id) => setProviders({ anthropicModel: id })} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="Cleanup provider">
        <SettingRow
          title="Who tidies up your text"
          description="Claude is recommended. You can switch to OpenAI instead if you would rather use only one account."
          stacked
        >
          <Segmented<CleanupProvider>
            value={settings.providers.cleanupProvider}
            options={[
              { value: "anthropic", label: "Claude" },
              { value: "openAi", label: "OpenAI" },
            ]}
            onChange={(cleanupProvider) => setProviders({ cleanupProvider })}
          />
        </SettingRow>
        {settings.providers.cleanupProvider === "openAi" && (
          <SettingRow title="OpenAI cleanup model" description="The OpenAI model used for cleanup when OpenAI is chosen above." stacked>
            <ModelSelect label="OpenAI cleanup model" models={openai} value={settings.providers.openaiCleanupModel} onChange={(id) => setProviders({ openaiCleanupModel: id })} />
          </SettingRow>
        )}
      </SettingGroup>

      <p className="muted section-note">
        Keys are stored in Windows Credential Manager, never in files. Rough costs: speech-to-text is about $0.27 per hour of speaking.
        Cleanup with Claude Sonnet is roughly a few dollars a month for typical use.
      </p>
    </>
  );
}
