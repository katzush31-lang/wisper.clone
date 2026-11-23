// The header band across the top of the window.
// Row 1: brand, "Jump to" search, theme, help, window buttons (the whole row drags the window).
// Row 2: every page as a tab; the open tab joins the page below it.
import { CircleHelp, Minus, Moon, Search, Square, Sun, X } from "lucide-react";
import { currentWindow } from "../shared/tauri";
import { PAGES, PAGE_ORDER, SETTINGS_PAGE, type NavPage, type PageId } from "./nav";

export function Logo() {
  return (
    <div className="logo">
      <span className="logo-mark" aria-hidden>
        <svg width="14" height="14" viewBox="0 0 14 14">
          {[5, 10, 14, 8].map((h, i) => (
            <rect key={i} x={i * 3.6} y={(14 - h) / 2} width="2.2" height={h} rx="1.1" fill="currentColor" />
          ))}
        </svg>
      </span>
      <span>Wisper</span>
    </div>
  );
}

interface Props {
  dark: boolean;
  page: PageId;
  onNavigate: (p: PageId) => void;
  onToggleTheme: () => void;
  onOpenJump: () => void;
  onOpenHelp: () => void;
}

function Tab({ p, page, onNavigate }: { p: NavPage; page: PageId; onNavigate: (p: PageId) => void }) {
  const n = PAGE_ORDER.indexOf(p.id);
  const keyHint = p.id === "settings" ? "Ctrl+," : `Ctrl+${n + 1}`;
  return (
    <button
      className="tabnav-item"
      aria-current={page === p.id ? "page" : undefined}
      title={`${p.hint} (${keyHint})`}
      onClick={() => onNavigate(p.id)}
    >
      {p.icon(16)}
      <span>{p.label}</span>
    </button>
  );
}

export default function TitleBar({ dark, page, onNavigate, onToggleTheme, onOpenJump, onOpenHelp }: Props) {
  const act = async (fn: "minimize" | "toggleMaximize" | "close") => {
    const w = await currentWindow();
    if (!w) return;
    if (fn === "close") await w.hide();
    else await w[fn]();
  };
  return (
    <header className="appbar">
      <div className="appbar-top" data-tauri-drag-region>
        <Logo />
        <div className="appbar-drag" data-tauri-drag-region />
        <button className="jump-btn" onClick={onOpenJump} aria-keyshortcuts="Control+K" title="Find any page or setting (Ctrl+K)">
          <Search size={15} aria-hidden />
          <span>Jump to…</span>
          <kbd>Ctrl K</kbd>
        </button>
        <button
          className="band-btn"
          aria-label={dark ? "Switch to light theme" : "Switch to dark theme"}
          title={dark ? "Light theme" : "Dark theme"}
          onClick={onToggleTheme}
        >
          {dark ? <Sun size={17} /> : <Moon size={17} />}
        </button>
        <button className="band-btn" aria-label="Help" title="Open the guide in your browser" onClick={onOpenHelp}>
          <CircleHelp size={17} />
        </button>
        <span className="appbar-gap" />
        <button className="band-btn win-btn" aria-label="Minimise" title="Minimise" onClick={() => act("minimize")}>
          <Minus size={16} />
        </button>
        <button className="band-btn win-btn" aria-label="Maximise" title="Maximise" onClick={() => act("toggleMaximize")}>
          <Square size={13} />
        </button>
        <button className="band-btn win-btn win-close" aria-label="Close to tray" title="Close to tray" onClick={() => act("close")}>
          <X size={17} />
        </button>
      </div>
      <nav className="tabnav" aria-label="Pages">
        {PAGES.filter((p) => p.group === "main").map((p) => (
          <Tab key={p.id} p={p} page={page} onNavigate={onNavigate} />
        ))}
        <span className="tabnav-sep" aria-hidden />
        {PAGES.filter((p) => p.group === "teach").map((p) => (
          <Tab key={p.id} p={p} page={page} onNavigate={onNavigate} />
        ))}
        <span className="tabnav-fill" data-tauri-drag-region />
        <Tab p={SETTINGS_PAGE} page={page} onNavigate={onNavigate} />
      </nav>
    </header>
  );
}
