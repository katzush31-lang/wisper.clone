// Ctrl+K "Jump to": type a few letters, press Enter, land on any page or settings section.
import { CornerDownLeft, Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { rankJump } from "./jump";

export interface JumpItem {
  id: string;
  label: string;
  hint: string;
  keywords?: string;
  icon: ReactNode;
  /** small grey word on the right, e.g. "Page" or "Setting" */
  kind: string;
  run: () => void;
}

export default function JumpTo({ items, onClose }: { items: JumpItem[]; onClose: () => void }) {
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const listRef = useRef<HTMLUListElement>(null);
  const results = useMemo(() => rankJump(items, q), [items, q]);

  useEffect(() => setSel(0), [q]);
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-i="${sel}"]`)?.scrollIntoView({ block: "nearest" });
  }, [sel]);

  const choose = (it: JumpItem | undefined) => {
    if (!it) return;
    onClose();
    it.run();
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSel((s) => Math.min(s + 1, results.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSel((s) => Math.max(s - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(results[sel]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    }
  };

  const active = results[sel] ? `jump-opt-${results[sel].id}` : undefined;

  return (
    <div className="jump-backdrop" onMouseDown={onClose}>
      <div className="jump" role="dialog" aria-modal aria-label="Jump to a page or setting" onMouseDown={(e) => e.stopPropagation()}>
        <div className="jump-input">
          <Search size={17} aria-hidden />
          <input
            autoFocus
            role="combobox"
            aria-expanded
            aria-controls="jump-list"
            aria-activedescendant={active}
            aria-label="Type to find a page or setting"
            placeholder="Type to find a page or setting, e.g. “keys” or “theme”"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={onKey}
          />
          <kbd>Esc</kbd>
        </div>
        <ul className="jump-list" id="jump-list" role="listbox" ref={listRef} aria-label="Results">
          {results.length === 0 && <li className="jump-empty">Nothing matches “{q}”. Try a shorter word.</li>}
          {results.map((it, i) => (
            <li
              key={it.id}
              id={`jump-opt-${it.id}`}
              data-i={i}
              role="option"
              aria-selected={i === sel}
              className="jump-opt"
              onMouseMove={() => setSel(i)}
              onClick={() => choose(it)}
            >
              <span className="jump-icon" aria-hidden>
                {it.icon}
              </span>
              <span className="jump-text">
                <span className="jump-label">{it.label}</span>
                <span className="jump-hint">{it.hint}</span>
              </span>
              <span className="jump-kind">{it.kind}</span>
              {i === sel && <CornerDownLeft size={14} className="jump-enter" aria-hidden />}
            </li>
          ))}
        </ul>
        <div className="jump-foot">
          <span>
            <kbd>↑</kbd> <kbd>↓</kbd> to move
          </span>
          <span>
            <kbd>Enter</kbd> to open
          </span>
          <span>
            <kbd>Ctrl</kbd> + <kbd>1</kbd>–<kbd>7</kbd> for pages
          </span>
        </div>
      </div>
    </div>
  );
}
