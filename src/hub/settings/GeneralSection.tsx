import { useEffect, useRef, useState } from "react";
import { api, events, type Microphone, type Settings, type Theme } from "../../shared/api";
import { Segmented, SettingGroup, SettingRow, Switch, errorText, useToast } from "../components";
import { languageChoice, languagesFor, type LanguageChoice } from "./helpers";

export interface SectionProps {
  settings: Settings;
  update: (next: Settings) => Promise<void>;
}

/** Saves and shows a toast if saving fails. */
export function useSaver(update: SectionProps["update"]) {
  const toast = useToast();
  return (next: Settings) => update(next).catch((e) => toast(errorText(e), "error"));
}

function MicTest({ deviceId }: { deviceId: string | null }) {
  const toast = useToast();
  const [running, setRunning] = useState(false);
  const [level, setLevel] = useState(0);
  const runningRef = useRef(false);

  useEffect(() => {
    let off: (() => void) | undefined;
    let dead = false;
    events.micLevel((l) => setLevel(l)).then((u) => (dead ? u() : (off = u)));
    return () => {
      dead = true;
      off?.();
      if (runningRef.current) api.stopMicTest().catch(() => {});
    };
  }, []);

  // If the chosen microphone changes mid-test, stop the test.
  useEffect(() => {
    if (runningRef.current) {
      runningRef.current = false;
      setRunning(false);
      setLevel(0);
      api.stopMicTest().catch(() => {});
    }
  }, [deviceId]);

  const toggle = async () => {
    try {
      if (running) {
        runningRef.current = false;
        setRunning(false);
        setLevel(0);
        await api.stopMicTest();
      } else {
        await api.startMicTest(deviceId);
        runningRef.current = true;
        setRunning(true);
      }
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <div className="mic-test">
      {running && (
        <div className="level-bar" role="meter" aria-label="Microphone level" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(level * 100)}>
          <div className="level-fill" style={{ width: `${Math.min(100, Math.round(level * 140))}%` }} />
        </div>
      )}
      <button className="btn btn-soft" onClick={toggle}>
        {running ? "Stop" : "Test"}
      </button>
    </div>
  );
}

export default function GeneralSection({ settings, update }: SectionProps) {
  const save = useSaver(update);
  const [name, setName] = useState(settings.userName);
  const [mics, setMics] = useState<Microphone[] | null>(null);

  useEffect(() => setName(settings.userName), [settings.userName]);
  useEffect(() => {
    api
      .listMicrophones()
      .then(setMics)
      .catch(() => setMics([]));
  }, []);

  const commitName = () => {
    const v = name.trim();
    if (v !== settings.userName) void save({ ...settings, userName: v });
  };

  const selectedMic = mics?.find((m) => m.id === settings.microphone);
  const micMissing = settings.microphone !== null && mics !== null && mics.length > 0 && !selectedMic;

  return (
    <>
      <SettingGroup title="You">
        <SettingRow title="Your name" description="Used to greet you on the home page. Nothing else.">
          <input
            className="input settings-input"
            value={name}
            placeholder="Your name"
            onChange={(e) => setName(e.target.value)}
            onBlur={commitName}
            onKeyDown={(e) => e.key === "Enter" && (e.currentTarget.blur())}
          />
        </SettingRow>
        <SettingRow title="Theme" description="Light, dark, or follow whatever Windows is set to.">
          <Segmented<Theme>
            value={settings.theme}
            options={[
              { value: "system", label: "System" },
              { value: "light", label: "Light" },
              { value: "dark", label: "Dark" },
            ]}
            onChange={(theme) => save({ ...settings, theme })}
          />
        </SettingRow>
        <SettingRow title="Launch at login" description="Starts Wisper quietly in the background when you sign in to Windows, so it is always ready.">
          <Switch label="Launch at login" checked={settings.launchAtLogin} onChange={(v) => save({ ...settings, launchAtLogin: v })} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="Microphone">
        <SettingRow
          title="Microphone"
          description={
            <>
              Which microphone Wisper listens to. "Windows default" follows whatever you choose in Windows sound settings.
              {selectedMic?.bluetooth && (
                <span className="setting-warn">
                  This is a Bluetooth microphone. Bluetooth headsets are slower to start and the sound quality is lower, so the first
                  word may get cut off and accuracy can suffer. A wired or built-in mic works best.
                </span>
              )}
              {micMissing && (
                <span className="setting-warn">The microphone you picked isn't plugged in right now, so Windows default is used instead.</span>
              )}
            </>
          }
        >
          <select
            className="select settings-select"
            value={settings.microphone ?? ""}
            onChange={(e) => save({ ...settings, microphone: e.target.value || null })}
            aria-label="Microphone"
          >
            <option value="">Windows default</option>
            {micMissing && <option value={settings.microphone ?? ""}>(Not connected)</option>}
            {(mics ?? []).map((m) => (
              <option key={m.id} value={m.id}>
                {m.name}
                {m.isDefault ? " (current default)" : ""}
                {m.bluetooth ? " (Bluetooth)" : ""}
              </option>
            ))}
          </select>
          <MicTest deviceId={settings.microphone} />
        </SettingRow>
        <SettingRow
          title="Keep microphone ready"
          description="Keeps the microphone switched on so the very first syllable is never missed. The downside: Windows will show the microphone icon all the time while Wisper is running."
        >
          <Switch label="Keep microphone ready" checked={settings.keepMicWarm} onChange={(v) => save({ ...settings, keepMicWarm: v })} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="Language">
        <SettingRow
          title="Languages you speak"
          description='"English" is fastest and most accurate. "English + Hebrew" lets you mix in Hebrew. "Auto-detect" works out the language each time. Hebrew and Yiddish words are always written in English letters (like Shabbos or zechus), whichever you pick.'
          stacked
        >
          <Segmented<LanguageChoice>
            value={languageChoice(settings.languages)}
            options={[
              { value: "en", label: "English" },
              { value: "en-he", label: "English + Hebrew" },
              { value: "auto", label: "Auto-detect" },
            ]}
            onChange={(c) => save({ ...settings, languages: languagesFor(c) })}
          />
        </SettingRow>
      </SettingGroup>
    </>
  );
}
