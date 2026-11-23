import { useCallback, useEffect, useMemo, useState, type ReactElement } from "react";
import { CircleHelp, Moon } from "lucide-react";
import TitleBar from "./TitleBar";
import JumpTo, { type JumpItem } from "./JumpTo";
import HomePage from "./pages/HomePage";
import InsightsPage from "./pages/InsightsPage";
import DictionaryPage from "./pages/DictionaryPage";
import SnippetsPage from "./pages/SnippetsPage";
import StylePage from "./pages/StylePage";
import TransformsPage from "./pages/TransformsPage";
import ScratchpadPage from "./pages/ScratchpadPage";
import SettingsPage from "./settings/SettingsPage";
import { PAGES, PAGE_ORDER, SETTINGS_SECTIONS, pageLabel, validSection, type PageId } from "./nav";
import { api, events, type Settings } from "../shared/api";
import { openUrl } from "../shared/tauri";
import { SettingsContext } from "./useSettings";
import { ToastProvider } from "./components";

const HELP_URL = "https://github.com/katzush31-lang/wisper.clone#readme";

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
  const [section, setSection] = useState("general");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [dark, setDark] = useState(() => applyTheme(undefined));
  const [jumpOpen, setJumpOpen] = useState(false);
  // While a settings control (shortcut capture) is listening for keys, app-wide shortcuts stay off.
  const [captureBusy, setCaptureBusy] = useState(false);

  const openSettings = useCallback((s?: string) => {
    setSection(validSection(s));
    setPage("settings");
  }, []);

  useEffect(() => {
    api.getSettings().then(setSettings).catch(() => {});
    const unsubs = [
      events.settingsChanged(setSettings),
      events.navigate((p) => {
        if (p.startsWith("settings")) openSettings(p.split(":")[1]);
        else setPage(p as PageId);
      }),
    ];
    return () => {
      unsubs.forEach((u) => u.then((f) => f()));
    };
  }, [openSettings]);

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

  // Keyboard: Ctrl+K jump to, Ctrl+1…7 pages, Ctrl+, settings.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (captureBusy || !e.ctrlKey || e.altKey || e.metaKey) return;
      if (e.key === "k" || e.key === "K") {
        e.preventDefault();
        setJumpOpen((o) => !o);
      } else if (e.key === ",") {
        e.preventDefault();
        openSettings(section);
      } else if (/^[1-9]$/.test(e.key) && !e.shiftKey) {
        const target = PAGE_ORDER[Number(e.key) - 1];
        if (target) {
          e.preventDefault();
          setJumpOpen(false);
          setPage(target);
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [captureBusy, openSettings, section]);

  const jumpItems = useMemo<JumpItem[]>(
    () => [
      ...PAGES.map((p) => ({
        id: p.id,
        label: p.label,
        hint: p.hint,
        keywords: p.keywords,
        icon: p.icon(16),
        kind: "Page",
        run: () => setPage(p.id),
      })),
      ...SETTINGS_SECTIONS.map((s) => ({
        id: `settings:${s.id}`,
        label: s.label,
        hint: s.hint,
        keywords: `settings ${s.keywords ?? ""}`,
        icon: s.icon,
        kind: "Setting",
        run: () => openSettings(s.id),
      })),
      {
        id: "action:theme",
        label: dark ? "Use light theme" : "Use dark theme",
        hint: "Switch the colours of this window",
        keywords: "theme dark light mode colours",
        icon: <Moon size={16} />,
        kind: "Action",
        run: toggleTheme,
      },
      {
        id: "action:help",
        label: "Help",
        hint: "Open the guide in your browser",
        keywords: "guide readme how",
        icon: <CircleHelp size={16} />,
        kind: "Action",
        run: () => openUrl(HELP_URL),
      },
    ],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [dark, settings, openSettings],
  );

  const pages: Record<Exclude<PageId, "settings">, ReactElement> = {
    home: <HomePage onOpenSettings={openSettings} />,
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
          <a className="skip-link" href="#main">
            Skip to content
          </a>
          <TitleBar
            dark={dark}
            page={page}
            onNavigate={setPage}
            onToggleTheme={toggleTheme}
            onOpenJump={() => setJumpOpen(true)}
            onOpenHelp={() => openUrl(HELP_URL)}
          />
          <main id="main" className="content scroll" aria-label={pageLabel(page)}>
            {page === "settings" ? (
              <SettingsPage section={section} onSection={setSection} onBusy={setCaptureBusy} />
            ) : (
              pages[page]
            )}
          </main>
          {jumpOpen && <JumpTo items={jumpItems} onClose={() => setJumpOpen(false)} />}
        </div>
      </ToastProvider>
    </SettingsContext.Provider>
  );
}
