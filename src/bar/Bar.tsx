import { Check, ChevronUp, Copy, Mic, RotateCcw, Settings, Undo2, X, History, Wand2 } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { api, events, formatDuration, type BarState } from "../shared/api";
import { barHeight, pushLevel } from "./waveform";

const BARS = 12;

const HIDDEN: BarState = {
  phase: "hidden",
  level: 0,
  elapsedMs: 0,
  handsFree: false,
  kind: "dictation",
  message: null,
  detail: null,
  text: null,
  dictationId: null,
  actions: [],
  modeName: null,
  modes: [],
  hotkeyLabel: "",
};

/** Run a bar command; the bar has nowhere to show errors, so ignore failures. */
function run(p: Promise<unknown>) {
  p.catch(() => {});
}

function Waveform({ levels }: { levels: number[] }) {
  return (
    <div className="wave" aria-hidden>
      {levels.map((l, i) => (
        <span key={i} className="wave-bar" style={{ height: barHeight(l) }} />
      ))}
    </div>
  );
}

function RoundBtn({
  label,
  onClick,
  kind,
  children,
}: {
  label: string;
  onClick: () => void;
  kind?: "ok" | "cancel";
  children: ReactNode;
}) {
  return (
    <button type="button" className={`round-btn ${kind ?? ""}`} aria-label={label} title={label} onClick={onClick}>
      {children}
    </button>
  );
}

function PillButton({ label, onClick, icon }: { label: string; onClick: () => void; icon: ReactNode }) {
  return (
    <button type="button" className="pill-btn" onClick={onClick}>
      {icon}
      {label}
    </button>
  );
}

export default function Bar() {
  const [s, setS] = useState<BarState>(HIDDEN);
  const [levels, setLevels] = useState<number[]>(() => Array(BARS).fill(0));
  const [menuOpen, setMenuOpen] = useState(false);
  const phaseRef = useRef(s.phase);

  useEffect(() => {
    document.body.classList.add("bar-body");
    const un = events.barState((next) => {
      if (next.phase !== phaseRef.current) {
        phaseRef.current = next.phase;
        setMenuOpen(false);
        if (next.phase !== "listening") setLevels(Array(BARS).fill(0));
      }
      if (next.phase === "listening") setLevels((h) => pushLevel(h, next.level, BARS));
      setS(next);
    });
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  }, []);

  const has = (a: string) => s.actions.includes(a);
  const key = `${s.phase}-${s.kind}`;

  let pill: ReactNode = null;
  let above: ReactNode = null;

  switch (s.phase) {
    case "idle":
      pill = (
        <div key={key} className="pill idle enter">
          <button type="button" className="idle-mic" aria-label="Start dictating" onClick={() => run(api.barStart())}>
            <Mic size={14} />
          </button>
          <span className="idle-hint">Hold {s.hotkeyLabel || "your shortcut"} to dictate</span>
        </div>
      );
      break;

    case "listening":
      above = s.handsFree ? (
        <div key="tip" className="tip enter">
          Press {s.hotkeyLabel || "the shortcut"} to finish and paste
        </div>
      ) : null;
      pill = (
        <div key={key} className={`pill listening enter ${s.kind === "command" ? "command" : ""}`}>
          <RoundBtn label="Cancel" kind="cancel" onClick={() => run(api.barCancel())}>
            <X size={14} />
          </RoundBtn>
          {s.kind === "command" && <span className="pill-tag">Command</span>}
          <Waveform levels={levels} />
          {s.elapsedMs > 10_000 && <span className="pill-time">{formatDuration(s.elapsedMs)}</span>}
          <RoundBtn label="Finish and paste" kind="ok" onClick={() => run(api.barFinish())}>
            <Check size={14} />
          </RoundBtn>
        </div>
      );
      break;

    case "processing":
      pill = (
        <div key={key} className="pill processing enter" role="status">
          <span className="dots" aria-hidden>
            <i />
            <i />
            <i />
          </span>
          <span className="pill-msg shimmer">{s.message ?? "Working on it…"}</span>
        </div>
      );
      break;

    case "done":
      pill = (
        <div key={key} className="pill done enter" role="status">
          <span className="done-check" aria-hidden>
            <Check size={13} strokeWidth={3} />
          </span>
          <span className="pill-msg">{s.message ?? "Done"}</span>
          {has("undo") && <PillButton label="Undo" icon={<Undo2 size={12} />} onClick={() => run(api.barUndo())} />}
          {has("rerun") && s.modes.length > 0 && (
            <PillButton
              label="Redo as"
              icon={<ChevronUp size={12} />}
              onClick={() => setMenuOpen((o) => !o)}
            />
          )}
          {has("copy") && <PillButton label="Copy" icon={<Copy size={12} />} onClick={() => run(api.barCopy())} />}
        </div>
      );
      above =
        menuOpen && s.modes.length > 0 ? (
          <div key="menu" className="menu enter" role="menu">
            {s.modes.map((m) => (
              <button
                key={m.id}
                type="button"
                role="menuitem"
                className="menu-item"
                onClick={() => {
                  setMenuOpen(false);
                  run(api.barRerun(m.id));
                }}
              >
                <Wand2 size={12} />
                {m.name}
              </button>
            ))}
          </div>
        ) : null;
      break;

    case "error":
    case "notice": {
      const isError = s.phase === "error";
      above = (
        <div key={key} className={`card-msg enter ${isError ? "error" : "notice"}`} role={isError ? "alert" : "status"}>
          <div className="card-head">
            <div className="card-title">{s.message ?? (isError ? "Something went wrong" : "Heads up")}</div>
            <button type="button" className="card-x" aria-label="Dismiss" onClick={() => run(api.barDismiss())}>
              <X size={14} />
            </button>
          </div>
          {s.detail && <div className="card-detail">{s.detail}</div>}
          {isError && s.actions.length > 0 && (
            <div className="card-actions">
              {has("retry") && <PillButton label="Try again" icon={<RotateCcw size={12} />} onClick={() => run(api.barRetry())} />}
              {has("copy") && <PillButton label="Copy text" icon={<Copy size={12} />} onClick={() => run(api.barCopy())} />}
              {has("settings") && (
                <PillButton label="Settings" icon={<Settings size={12} />} onClick={() => run(api.barOpenHub("settings:providers"))} />
              )}
              {has("history") && (
                <PillButton label="History" icon={<History size={12} />} onClick={() => run(api.barOpenHub("home"))} />
              )}
            </div>
          )}
        </div>
      );
      pill = <div key={key} className={`pill idle enter ${isError ? "err-dot" : ""}`} aria-hidden />;
      break;
    }

    default:
      break;
  }

  return (
    <div className="bar-root">
      {above && <div className="bar-above">{above}</div>}
      {pill}
    </div>
  );
}
