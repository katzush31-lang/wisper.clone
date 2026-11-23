import { useEffect, useState } from "react";
import { api, type AppInfoDto } from "../../shared/api";
import { openUrl } from "../../shared/tauri";
import { SettingGroup, SettingRow, errorText, useToast } from "../components";
import { AUDIO_RETENTION, HISTORY_RETENTION, retentionFromString, retentionToString, withCurrent, type Option } from "./helpers";
import { useSaver, type SectionProps } from "./GeneralSection";

function RetentionSelect({ options, value, onChange, label }: { options: Option<number | null>[]; value: number | null; onChange: (v: number | null) => void; label: string }) {
  return (
    <select className="select settings-select" aria-label={label} value={retentionToString(value)} onChange={(e) => onChange(retentionFromString(e.target.value))}>
      {withCurrent(options, value).map((o) => (
        <option key={retentionToString(o.value)} value={retentionToString(o.value)}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

export function DataSection({ settings, update }: SectionProps) {
  const save = useSaver(update);
  const toast = useToast();
  return (
    <>
      <SettingGroup title="What is kept on this PC">
        <SettingRow
          title="Keep audio recordings"
          description="How long your voice recordings are kept so you can replay or retry them. Older ones are deleted automatically. Audio takes up about 1 MB per minute."
        >
          <RetentionSelect label="Keep audio recordings" options={AUDIO_RETENTION} value={settings.keepAudioDays} onChange={(keepAudioDays) => save({ ...settings, keepAudioDays })} />
        </SettingRow>
        <SettingRow title="Keep history" description="How long the text of your past dictations stays in your History list. Older ones are deleted automatically.">
          <RetentionSelect label="Keep history" options={HISTORY_RETENTION} value={settings.keepHistoryDays} onChange={(keepHistoryDays) => save({ ...settings, keepHistoryDays })} />
        </SettingRow>
        <SettingRow title="Open data folder" description="Shows the folder on this PC where Wisper keeps your recordings, history and settings.">
          <button className="btn btn-soft" onClick={() => api.openDataFolder().catch((e) => toast(errorText(e), "error"))}>
            Open folder
          </button>
        </SettingRow>
      </SettingGroup>
      <SettingGroup title="What leaves your PC">
        <div className="privacy-note">
          <p>
            <strong>Your voice recording</strong> is sent to OpenAI to be turned into text.
          </p>
          <p>
            <strong>The text</strong> is sent to Claude (or OpenAI, if you chose that) to be tidied up. If "Read text around the cursor" is on, that includes the app name, window title and a little text before your cursor.
          </p>
          <p>Nothing else leaves your PC. Signing in to a Wisper account is optional. Without one, no one else can see your history.</p>
        </div>
      </SettingGroup>
    </>
  );
}

export function AboutSection() {
  const toast = useToast();
  const [info, setInfo] = useState<AppInfoDto | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);

  useEffect(() => {
    api.appInfo().then(setInfo).catch(() => {});
  }, []);

  const check = async () => {
    setChecking(true);
    setMessage(null);
    try {
      setMessage((await api.checkForUpdates()).message);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setChecking(false);
    }
  };

  return (
    <SettingGroup>
      <SettingRow title="Version" description="The version of Wisper you are running.">
        <span>{info?.version ?? "Unknown"}</span>
      </SettingRow>
      <SettingRow title="Check for updates" description={message ?? "Wisper also checks by itself in the background and tells you when an update is ready."}>
        <button className="btn btn-soft" disabled={checking} onClick={check}>
          {checking ? "Checking…" : "Check for updates"}
        </button>
      </SettingRow>
      <SettingRow title="Release notes" description="See what changed in each version on GitHub.">
        <button className="btn btn-soft" onClick={() => void openUrl("https://github.com/katzush31-lang/wisper.clone/releases")}>
          Open releases page
        </button>
      </SettingRow>
    </SettingGroup>
  );
}
