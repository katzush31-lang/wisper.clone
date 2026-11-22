import { Minus, Moon, PanelLeft, Square, Sun, X } from "lucide-react";
import { currentWindow } from "../shared/tauri";

interface Props {
  dark: boolean;
  onToggleTheme: () => void;
  onToggleSidebar: () => void;
}

// Custom title bar matching Wispr Flow: sidebar toggle on the left, theme + window controls on the right.
export default function TitleBar({ dark, onToggleTheme, onToggleSidebar }: Props) {
  const act = async (fn: "minimize" | "toggleMaximize" | "close") => {
    const w = await currentWindow();
    if (!w) return;
    if (fn === "close") await w.hide();
    else await w[fn]();
  };
  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="titlebar-left">
        <button className="icon-btn" title="Toggle sidebar" onClick={onToggleSidebar}>
          <PanelLeft size={18} />
        </button>
      </div>
      <div className="titlebar-right">
        <button className="icon-btn" title={dark ? "Light theme" : "Dark theme"} onClick={onToggleTheme}>
          {dark ? <Sun size={17} /> : <Moon size={17} />}
        </button>
        <span className="titlebar-gap" />
        <button className="icon-btn win-btn" title="Minimise" onClick={() => act("minimize")}>
          <Minus size={16} />
        </button>
        <button className="icon-btn win-btn" title="Maximise" onClick={() => act("toggleMaximize")}>
          <Square size={13} />
        </button>
        <button className="icon-btn win-btn win-close" title="Close to tray" onClick={() => act("close")}>
          <X size={17} />
        </button>
      </div>
    </div>
  );
}
