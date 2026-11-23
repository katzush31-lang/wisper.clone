// Settings as a full page (not a pop-up): section list with plain-English hints on the left,
// the chosen section on the right.
import { useEffect, useRef, type ReactNode } from "react";
import { useSettings } from "../useSettings";
import { SETTINGS_SECTIONS, validSection } from "../nav";
import GeneralSection from "./GeneralSection";
import ShortcutsSection from "./ShortcutsSection";
import { AudioSection, DictationSection, NotificationsSection } from "./DictationSection";
import ProvidersSection from "./ProvidersSection";
import { AboutSection, DataSection } from "./DataSections";
import AccountSection from "./AccountSection";
import "./settings.css";

interface Props {
  section: string;
  onSection: (id: string) => void;
  /** true while a child (e.g. shortcut capture) needs every key press */
  onBusy: (busy: boolean) => void;
}

export default function SettingsPage({ section: raw, onSection, onBusy }: Props) {
  const { settings, update } = useSettings();
  const section = validSection(raw);
  const mainRef = useRef<HTMLDivElement>(null);
  const headRef = useRef<HTMLHeadingElement>(null);
  const first = useRef(true);

  useEffect(() => {
    mainRef.current?.scrollTo({ top: 0 });
    // Move keyboard focus to the new section's heading (not on first load), so screen readers announce it.
    if (first.current) first.current = false;
    else headRef.current?.focus();
  }, [section]);

  const current = SETTINGS_SECTIONS.find((s) => s.id === section)!;

  let body: ReactNode;
  if (!settings) {
    body = <p className="muted">Settings are only available in the app. Loading…</p>;
  } else {
    const p = { settings, update };
    switch (section) {
      case "general":
        body = <GeneralSection {...p} />;
        break;
      case "shortcuts":
        body = <ShortcutsSection {...p} onBusy={onBusy} />;
        break;
      case "dictation":
        body = <DictationSection {...p} />;
        break;
      case "audio":
        body = <AudioSection {...p} />;
        break;
      case "notifications":
        body = <NotificationsSection {...p} />;
        break;
      case "providers":
        body = <ProvidersSection {...p} />;
        break;
      case "data":
        body = <DataSection {...p} />;
        break;
      case "account":
        body = <AccountSection />;
        break;
      default:
        body = <AboutSection />;
    }
  }

  return (
    <div className="settings-page">
      <nav className="settings-nav" aria-label="Settings sections">
        <h1 className="settings-nav-title">Settings</h1>
        {SETTINGS_SECTIONS.map((s) => (
          <button
            key={s.id}
            className="settings-nav-item"
            aria-current={s.id === section ? "page" : undefined}
            onClick={() => onSection(s.id)}
          >
            {s.icon}
            <span>
              <span className="settings-nav-label">{s.label}</span>
              <span className="settings-nav-hint">{s.hint}</span>
            </span>
          </button>
        ))}
      </nav>
      <div className="settings-main" ref={mainRef}>
        <div className="settings-inner">
          <header className="settings-head">
            <h2 ref={headRef} tabIndex={-1}>
              {current.label}
            </h2>
            <p>{current.hint}.</p>
          </header>
          {body}
        </div>
      </div>
    </div>
  );
}
