// The slim left rail: every page as an icon with its name underneath, grouped by what it's for.
import { CircleHelp } from "lucide-react";
import { PAGES, SETTINGS_PAGE, PAGE_ORDER, type NavPage, type PageId } from "./nav";

export type { PageId } from "./nav";

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
  page: PageId;
  onNavigate: (p: PageId) => void;
  onOpenHelp: () => void;
}

function RailItem({ p, page, onNavigate }: { p: NavPage; page: PageId; onNavigate: (p: PageId) => void }) {
  const n = PAGE_ORDER.indexOf(p.id);
  const keyHint = p.id === "settings" ? "Ctrl+," : n >= 0 ? `Ctrl+${n + 1}` : "";
  return (
    <button
      className="rail-item"
      aria-current={page === p.id ? "page" : undefined}
      title={`${p.hint}${keyHint ? ` (${keyHint})` : ""}`}
      onClick={() => onNavigate(p.id)}
    >
      {p.icon(20)}
      <span>{p.label}</span>
    </button>
  );
}

export default function Sidebar({ page, onNavigate, onOpenHelp }: Props) {
  return (
    <nav className="rail" aria-label="Pages">
      <div className="rail-group">
        {PAGES.filter((p) => p.group === "main").map((p) => (
          <RailItem key={p.id} p={p} page={page} onNavigate={onNavigate} />
        ))}
      </div>
      <div className="rail-heading" id="rail-teach">
        Teach Wisper
      </div>
      <div className="rail-group" role="group" aria-labelledby="rail-teach">
        {PAGES.filter((p) => p.group === "teach").map((p) => (
          <RailItem key={p.id} p={p} page={page} onNavigate={onNavigate} />
        ))}
      </div>
      <div className="rail-spacer" />
      <div className="rail-group">
        <RailItem p={SETTINGS_PAGE} page={page} onNavigate={onNavigate} />
        <button className="rail-item" title="Open the guide in your browser" onClick={onOpenHelp}>
          <CircleHelp size={20} />
          <span>Help</span>
        </button>
      </div>
    </nav>
  );
}
