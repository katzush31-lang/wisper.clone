import { Bell, Cloud, Database, Info, Keyboard, KeyRound, Mic, Settings as Cog, Volume2, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useSettings } from "../useSettings";
import GeneralSection from "./GeneralSection";
import ShortcutsSection from "./ShortcutsSection";
import { AudioSection, DictationSection, NotificationsSection } from "./DictationSection";
import ProvidersSection from "./ProvidersSection";
import { AboutSection, DataSection } from "./DataSections";
import AccountSection from "./AccountSection";
import "./settings.css";

interface Props {
  open: boolean;
  initialSection?: string;
  onClose: () => void;
}

const SECTIONS: { id: string; label: string; icon: ReactNode }[] = [
  { id: "general", label: "General", icon: <Cog size={16} /> },
  { id: "shortcuts", label: "Shortcuts", icon: <Keyboard size={16} /> },
  { id: "dictation", label: "Dictation", icon: <Mic size={16} /> },
  { id: "audio", label: "Sounds & audio", icon: <Volume2 size={16} /> },
  { id: "notifications", label: "Notifications", icon: <Bell size={16} /> },
  { id: "providers", label: "AI providers", icon: <KeyRound size={16} /> },
  { id: "data", label: "Data & privacy", icon: <Database size={16} /> },
  { id: "account", label: "Account", icon: <Cloud size={16} /> },
  { id: "about", label: "About & updates", icon: <Info size={16} /> },
];

const validId = (id?: string) => (SECTIONS.some((s) => s.id === id) ? (id as string) : "general");

export default function SettingsModal({ open, initialSection, onClose }: Props) {
  const { settings, update } = useSettings();
  const [section, setSection] = useState(validId(initialSection));
  const [childBusy, setChildBusy] = useState(false);
  const paneRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (open) setSection(validId(initialSection));
  }, [open, initialSection]);

  useEffect(() => {
    paneRef.current?.scrollTo({ top: 0 });
  }, [section]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !childBusy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose, childBusy]);

  const onBusy = useCallback((b: boolean) => setChildBusy(b), []);

  if (!open) return null;
  const current = SECTIONS.find((s) => s.id === section)!;

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
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div className="settings-modal" role="dialog" aria-modal aria-label="Settings" onMouseDown={(e) => e.stopPropagation()}>
        <nav className="settings-nav" aria-label="Settings sections">
          <div className="settings-nav-title">Settings</div>
          {SECTIONS.map((s) => (
            <button
              key={s.id}
              className={`settings-nav-item ${s.id === section ? "active" : ""}`}
              aria-current={s.id === section ? "page" : undefined}
              onClick={() => setSection(s.id)}
            >
              {s.icon}
              {s.label}
            </button>
          ))}
        </nav>
        <div className="settings-main">
          <header className="settings-head">
            <h2>{current.label}</h2>
            <button className="icon-btn" onClick={onClose} aria-label="Close settings">
              <X size={18} />
            </button>
          </header>
          <div className="settings-pane" ref={paneRef}>
            {body}
          </div>
        </div>
      </div>
    </div>
  );
}
