import { useEffect, useState } from "react";
import { api, type BarPosition, type CleanupLevel, type Mode, type OtherAudio, type Settings } from "../../shared/api";
import { Segmented, SettingGroup, SettingRow, Switch } from "../components";
import { clampWpm, formatExcludedApps, parseExcludedApps } from "./helpers";
import { useSaver, type SectionProps } from "./GeneralSection";

export function DictationSection({ settings, update }: SectionProps) {
  const save = useSaver(update);
  const [modes, setModes] = useState<Mode[]>([]);
  const [excluded, setExcluded] = useState(formatExcludedApps(settings.contextExcludedApps));
  const [wpm, setWpm] = useState(String(settings.typingWpm));

  useEffect(() => {
    api
      .listModes()
      .then(setModes)
      .catch(() => setModes([]));
  }, []);
  useEffect(() => setExcluded(formatExcludedApps(settings.contextExcludedApps)), [settings.contextExcludedApps]);
  useEffect(() => setWpm(String(settings.typingWpm)), [settings.typingWpm]);

  const commitExcluded = () => {
    const list = parseExcludedApps(excluded);
    if (list.join(",") !== settings.contextExcludedApps.join(",")) void save({ ...settings, contextExcludedApps: list });
    else setExcluded(formatExcludedApps(list));
  };
  const commitWpm = () => {
    const n = clampWpm(Number(wpm));
    setWpm(String(n));
    if (n !== settings.typingWpm) void save({ ...settings, typingWpm: n });
  };
  const blurOnEnter = (e: React.KeyboardEvent<HTMLInputElement>) => e.key === "Enter" && e.currentTarget.blur();

  const hasCurrent = modes.some((m) => m.id === settings.defaultMode);

  return (
    <>
      <SettingGroup title="Writing">
        <SettingRow
          title="Default mode"
          description='The writing style used when nothing else applies. "Auto" picks a style that suits the app you are typing in.'
        >
          <select
            className="select settings-select"
            value={settings.defaultMode}
            onChange={(e) => save({ ...settings, defaultMode: e.target.value })}
            aria-label="Default mode"
          >
            {!hasCurrent && <option value={settings.defaultMode}>{settings.defaultMode}</option>}
            {modes.map((m) => (
              <option key={m.id} value={m.id}>
                {m.name}
              </option>
            ))}
          </select>
        </SettingRow>
        <SettingRow
          title="How much to edit"
          description="None: your words exactly as spoken. Light: fixes punctuation, capital letters and removes ums and false starts. Medium: also tidies the wording so it reads smoothly."
          stacked
        >
          <Segmented<CleanupLevel>
            value={settings.cleanupLevel}
            options={[
              { value: "none", label: "None" },
              { value: "light", label: "Light" },
              { value: "medium", label: "Medium" },
            ]}
            onChange={(cleanupLevel) => save({ ...settings, cleanupLevel })}
          />
        </SettingRow>
        <SettingRow
          title="Smart formatting"
          description='Understands spoken formatting such as "new line" and "new paragraph" and turns them into real line breaks.'
        >
          <Switch label="Smart formatting" checked={settings.smartFormatting} onChange={(v) => save({ ...settings, smartFormatting: v })} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="Context and privacy">
        <SettingRow
          title="Read text around the cursor"
          description="When on, each dictation also sends the app name, the window title and the text just before your cursor to the AI provider. This helps it spell names correctly and match your formatting. When off, only your speech is sent."
        >
          <Switch label="Read text around the cursor" checked={settings.contextAwareness} onChange={(v) => save({ ...settings, contextAwareness: v })} />
        </SettingRow>
        <SettingRow
          title="Excluded apps"
          description="Apps where Wisper never reads the text around your cursor, such as a password manager. Type the program names separated by commas, for example 1password.exe, keepass.exe."
          stacked
        >
          <input
            className="input"
            value={excluded}
            placeholder="1password.exe, keepass.exe"
            onChange={(e) => setExcluded(e.target.value)}
            onBlur={commitExcluded}
            onKeyDown={blurOnEnter}
            aria-label="Excluded apps"
          />
        </SettingRow>
        <SettingRow
          title="Learn per-app styles"
          description="Notices which writing style you use in each app and starts using it there automatically. You can always change these on the Style page."
        >
          <Switch label="Learn per-app styles" checked={settings.learnAppStyles} onChange={(v) => save({ ...settings, learnAppStyles: v })} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="Stats">
        <SettingRow
          title="Typing speed for “time saved”"
          description="How many words per minute you type by hand. Wisper uses this to work out how much time dictating saves you. Most people type 40 to 60."
        >
          <input
            className="input settings-input-small"
            type="number"
            min={10}
            max={250}
            value={wpm}
            onChange={(e) => setWpm(e.target.value)}
            onBlur={commitWpm}
            onKeyDown={blurOnEnter}
            aria-label="Typing speed in words per minute"
          />
          <span className="muted">words/min</span>
        </SettingRow>
      </SettingGroup>
    </>
  );
}

export function AudioSection({ settings, update }: SectionProps) {
  const save = useSaver(update);
  const pct = Math.round(settings.sounds.volume * 100);
  const audioDesc: Record<OtherAudio, string> = {
    off: "Off: leaves music, videos and calls alone while you dictate.",
    pause: "Pause: pauses music and videos while you speak and resumes them afterwards.",
    mute: "Mute: silences any app that is playing sound while you speak and puts its volume back afterwards.",
  };
  return (
    <>
      <SettingGroup title="Sounds">
        <SettingRow title="Sounds" description="Plays a short beep when recording starts and stops, so you know Wisper heard you.">
          <Switch
            label="Sounds"
            checked={settings.sounds.enabled}
            onChange={(enabled) => save({ ...settings, sounds: { ...settings.sounds, enabled } })}
          />
        </SettingRow>
        <SettingRow title="Volume" description="How loud those beeps are.">
          <input
            type="range"
            className="range"
            min={0}
            max={100}
            step={5}
            value={pct}
            disabled={!settings.sounds.enabled}
            aria-label="Sound volume"
            onChange={(e) => save({ ...settings, sounds: { ...settings.sounds, volume: Number(e.target.value) / 100 } })}
          />
          <span className="muted range-value">{pct}%</span>
        </SettingRow>
        <SettingRow title="Other audio while dictating" description={audioDesc[settings.otherAudio]} stacked>
          <Segmented<OtherAudio>
            value={settings.otherAudio}
            options={[
              { value: "off", label: "Off" },
              { value: "pause", label: "Pause" },
              { value: "mute", label: "Mute" },
            ]}
            onChange={(otherAudio) => save({ ...settings, otherAudio })}
          />
        </SettingRow>
      </SettingGroup>
      <SettingGroup title="The floating bar">
        <SettingRow
          title="Always show the bar"
          description="Keeps the small Wisper bar on screen all the time. When off, it only appears while you are dictating."
        >
          <Switch label="Always show the bar" checked={settings.alwaysShowBar} onChange={(v) => save({ ...settings, alwaysShowBar: v })} />
        </SettingRow>
        <SettingRow title="Bar position" description="Whether the bar sits at the bottom or the top of your screen.">
          <Segmented<BarPosition>
            value={settings.barPosition}
            options={[
              { value: "bottom", label: "Bottom" },
              { value: "top", label: "Top" },
            ]}
            onChange={(barPosition) => save({ ...settings, barPosition })}
          />
        </SettingRow>
      </SettingGroup>
    </>
  );
}

const NOTES: { key: keyof Settings["notifications"]; title: string; desc: string }[] = [
  { key: "dictationFailed", title: "Dictation failed", desc: "Tells you when something went wrong and a dictation could not be pasted. Your words are always saved so you can retry." },
  { key: "waitingOffline", title: "Waiting for internet", desc: "Tells you when there is no connection and a dictation is waiting to be sent. It finishes by itself once you are back online." },
  { key: "apiKeyProblem", title: "API key problem", desc: "Tells you if your OpenAI or Claude key stops working, for example if it expired or the account ran out of credit." },
  { key: "dictionarySuggestion", title: "Dictionary suggestions", desc: "Tells you when Wisper spots a word you keep fixing and suggests adding it to your dictionary." },
  { key: "updateReady", title: "Update ready", desc: "Tells you when a new version of Wisper has been downloaded and is ready to install." },
];

export function NotificationsSection({ settings, update }: SectionProps) {
  const save = useSaver(update);
  return (
    <>
      <SettingGroup>
        {NOTES.map((n) => (
          <SettingRow key={n.key} title={n.title} description={n.desc}>
            <Switch
              label={n.title}
              checked={settings.notifications[n.key]}
              onChange={(v) => save({ ...settings, notifications: { ...settings.notifications, [n.key]: v } })}
            />
          </SettingRow>
        ))}
      </SettingGroup>
      <p className="muted section-note">Successful dictations never send a notification. The bar already shows you that they worked.</p>
    </>
  );
}
