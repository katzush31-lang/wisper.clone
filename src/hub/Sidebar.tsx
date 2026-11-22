import {
  BarChart3,
  BookText,
  CircleHelp,
  FileText,
  Mic,
  Scissors,
  Settings,
  Type,
  Wand2,
} from "lucide-react";
import type { ReactNode } from "react";

export type PageId =
  | "home"
  | "insights"
  | "dictionary"
  | "snippets"
  | "style"
  | "transforms"
  | "scratchpad";

const NAV: { id: PageId; label: string; icon: ReactNode }[] = [
  { id: "home", label: "Dictation", icon: <Mic size={17} /> },
  { id: "insights", label: "Insights", icon: <BarChart3 size={17} /> },
  { id: "dictionary", label: "Dictionary", icon: <BookText size={17} /> },
  { id: "snippets", label: "Snippets", icon: <Scissors size={17} /> },
  { id: "style", label: "Style", icon: <Type size={17} /> },
  { id: "transforms", label: "Transforms", icon: <Wand2 size={17} /> },
  { id: "scratchpad", label: "Scratchpad", icon: <FileText size={17} /> },
];

export function Logo() {
  return (
    <div className="logo">
      <svg width="22" height="18" viewBox="0 0 22 18" aria-hidden>
        {[4, 9, 14, 7, 11].map((h, i) => (
          <rect key={i} x={i * 4.4} y={(18 - h) / 2} width="2.6" height={h} rx="1.3" fill="currentColor" />
        ))}
      </svg>
      <span>Wisper</span>
    </div>
  );
}

interface Props {
  page: PageId;
  onNavigate: (p: PageId) => void;
  onOpenSettings: () => void;
  onOpenHelp: () => void;
  footer?: ReactNode;
}

export default function Sidebar({ page, onNavigate, onOpenSettings, onOpenHelp, footer }: Props) {
  return (
    <nav className="sidebar">
      <Logo />
      <div className="nav-list">
        {NAV.map((n) => (
          <button
            key={n.id}
            className={`nav-item ${page === n.id ? "active" : ""}`}
            onClick={() => onNavigate(n.id)}
          >
            {n.icon}
            <span>{n.label}</span>
          </button>
        ))}
      </div>
      <div className="sidebar-spacer" />
      {footer}
      <div className="nav-list nav-bottom">
        <button className="nav-item" onClick={onOpenSettings}>
          <Settings size={17} />
          <span>Settings</span>
        </button>
        <button className="nav-item" onClick={onOpenHelp}>
          <CircleHelp size={17} />
          <span>Help</span>
        </button>
      </div>
    </nav>
  );
}
