import { useEffect, useState } from "react";
import Sidebar, { type PageId } from "./Sidebar";
import TitleBar from "./TitleBar";
import Placeholder from "./pages/Placeholder";
import { openUrl } from "../shared/tauri";

const TITLES: Record<PageId, [string, string]> = {
  home: ["Dictation", "Hold Ctrl + Win and speak. Your dictations will appear here."],
  insights: ["Insights", "Your usage stats will appear here."],
  dictionary: ["Dictionary", "Words and spellings Wisper should always get right."],
  snippets: ["Snippets", "Say a short phrase, get a longer piece of text."],
  style: ["Style", "Modes and per-app writing styles."],
  transforms: ["Transforms", "One-click rewrites for selected text."],
  scratchpad: ["Scratchpad", "A notes page you can dictate into."],
};

function useTheme(): [boolean, () => void] {
  const [dark, setDark] = useState(() =>
    typeof window !== "undefined" && window.matchMedia?.("(prefers-color-scheme: dark)").matches,
  );
  useEffect(() => {
    document.documentElement.dataset.theme = dark ? "dark" : "light";
  }, [dark]);
  return [dark, () => setDark((d) => !d)];
}

export default function App() {
  const [page, setPage] = useState<PageId>("home");
  const [sidebar, setSidebar] = useState(true);
  const [dark, toggleTheme] = useTheme();
  const [title, blurb] = TITLES[page];
  return (
    <div className="app">
      <TitleBar dark={dark} onToggleTheme={toggleTheme} onToggleSidebar={() => setSidebar((s) => !s)} />
      <div className="app-body">
        <div className={`sidebar-wrap ${sidebar ? "" : "collapsed"}`}>
          <Sidebar
            page={page}
            onNavigate={setPage}
            onOpenSettings={() => {}}
            onOpenHelp={() => openUrl("https://github.com/katzush31-lang/wisper.clone#readme")}
          />
        </div>
        <main className="content scroll">
          <Placeholder title={title} blurb={blurb} />
        </main>
      </div>
    </div>
  );
}
