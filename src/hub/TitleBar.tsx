import { Minus, Moon, Search, Square, Sun, X } from "lucide-react";
import { currentWindow } from "../shared/tauri";
import { Logo } from "./Sidebar";

interface Props {
  dark: boolean;
  pageTitle: string;
  onToggleTheme: () => void;
  onOpenJump: () => void;
}

// One bar across the top: brand + current page, a "Jump to" search in the middle, theme and window buttons on the right.
export default function TitleBar({ dark, pageTitle, onToggleTheme, onOpenJump }: Props) {
  const act = async (fn: "minimize" | "toggleMaximize" | "close") => {
    const w = await currentWindow();
    if (!w) return;
    if (fn === "close") await w.hide();
    else await w[fn]();
  };
  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="titlebar-left" data-tauri-drag-region>
        <Logo />
        <span className="titlebar-page" data-tauri-drag-region>
          {pageTitle}
        </span>
      </div>
      <button className="jump-btn" onClick={onOpenJump} aria-keyshortcuts="Control+K" title="Find any page or setting (Ctrl+K)">
        <Search size={15} aria-hidden />
        <span>Jump to a page or setting…</span>
        <kbd>Ctrl K</kbd>
      </button>
      <div className="titlebar-right" data-tauri-drag-region>
        <button
          className="icon-btn"
          aria-label={dark ? "Switch to light theme" : "Switch to dark theme"}
          title={dark ? "Light theme" : "Dark theme"}
          onClick={onToggleTheme}
        >
          {dark ? <Sun size={17} /> : <Moon size={17} />}
        </button>
        <span className="titlebar-gap" />
        <button className="icon-btn win-btn" aria-label="Minimise" title="Minimise" onClick={() => act("minimize")}>
          <Minus size={16} />
        </button>
        <button className="icon-btn win-btn" aria-label="Maximise" title="Maximise" onClick={() => act("toggleMaximize")}>
          <Square size={13} />
        </button>
        <button className="icon-btn win-btn win-close" aria-label="Close to tray" title="Close to tray" onClick={() => act("close")}>
          <X size={17} />
        </button>
      </div>
    </header>
  );
}
