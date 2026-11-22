import { useEffect, useState, type ReactElement } from "react";
import Sidebar, { type PageId } from "./Sidebar";
import TitleBar from "./TitleBar";
import HomePage from "./pages/HomePage";
import InsightsPage from "./pages/InsightsPage";
import DictionaryPage from "./pages/DictionaryPage";
import SnippetsPage from "./pages/SnippetsPage";
import StylePage from "./pages/StylePage";
import TransformsPage from "./pages/TransformsPage";
import ScratchpadPage from "./pages/ScratchpadPage";
import SettingsModal from "./settings/SettingsModal";
import { api, events, type Settings } from "../shared/api";
import { openUrl } from "../shared/tauri";
import { SettingsContext } from "./useSettings";
import { ToastProvider } from "./components";

function applyTheme(theme: Settings["theme"] | undefined) {
  const dark =
    theme === "dark" ||
    ((theme === undefined || theme === "system") &&
      window.matchMedia?.("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
  return !!dark;
}

export default function App() {
  const [page, setPage] = useState<PageId>("home");
  const [sidebar, setSidebar] = useState(true);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [settingsOpen, setSettingsOpen] = useState<string | null>(null);
  const [dark, setDark] = useState(() => applyTheme(undefined));

  useEffect(() => {
    api.getSettings().then(setSettings).catch(() => {});
    const unsubs = [
      events.settingsChanged(setSettings),
      events.navigate((p) => {
        if (p.startsWith("settings")) setSettingsOpen(p.split(":")[1] ?? "general");
        else setPage(p as PageId);
      }),
    ];
    return () => {
      unsubs.forEach((u) => u.then((f) => f()));
    };
  }, []);

  useEffect(() => {
    setDark(applyTheme(settings?.theme));
    const mq = window.matchMedia?.("(prefers-color-scheme: dark)");
    const onChange = () => setDark(applyTheme(settings?.theme));
    mq?.addEventListener?.("change", onChange);
    return () => mq?.removeEventListener?.("change", onChange);
  }, [settings?.theme]);

  const update = async (next: Settings) => {
    setSettings(next);
    try {
      setSettings(await api.saveSettings(next));
    } catch {
      /* keep optimistic value; errors are shown by the caller */
    }
  };

  const toggleTheme = () => {
    if (!settings) {
      setDark(applyTheme(dark ? "light" : "dark"));
      return;
    }
    update({ ...settings, theme: dark ? "light" : "dark" });
  };

  const pages: Record<PageId, ReactElement> = {
    home: <HomePage onOpenSettings={(s) => setSettingsOpen(s ?? "general")} />,
    insights: <InsightsPage />,
    dictionary: <DictionaryPage />,
    snippets: <SnippetsPage />,
    style: <StylePage />,
    transforms: <TransformsPage />,
    scratchpad: <ScratchpadPage />,
  };

  return (
    <SettingsContext.Provider value={{ settings, update }}>
      <ToastProvider>
      <div className="app">
        <TitleBar dark={dark} onToggleTheme={toggleTheme} onToggleSidebar={() => setSidebar((s) => !s)} />
        <div className="app-body">
          <div className={`sidebar-wrap ${sidebar ? "" : "collapsed"}`}>
            <Sidebar
              page={page}
              onNavigate={setPage}
              onOpenSettings={() => setSettingsOpen("general")}
              onOpenHelp={() => openUrl("https://github.com/katzush31-lang/wisper.clone#readme")}
            />
          </div>
          <main className="content scroll">{pages[page]}</main>
        </div>
        <SettingsModal
          open={settingsOpen !== null}
          initialSection={settingsOpen ?? undefined}
          onClose={() => setSettingsOpen(null)}
        />
      </div>
      </ToastProvider>
    </SettingsContext.Provider>
  );
}
