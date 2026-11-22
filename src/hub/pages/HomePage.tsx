import { Mic, Search } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, compactNumber, comboLabel, events, type HistoryItem, type Stats } from "../../shared/api";
import { EmptyState, SearchBox, dayLabel, timeLabel } from "../components";
import { useSettings } from "../useSettings";
import HistoryDetail from "./home/HistoryDetail";
import { itemText, statusInfo } from "./home/helpers";
import "./home.css";

const PAGE = 50;

export default function HomePage({ onOpenSettings }: { onOpenSettings: (section?: string) => void }) {
  const { settings } = useSettings();
  const [stats, setStats] = useState<Stats | null>(null);
  const [keysMissing, setKeysMissing] = useState(false);
  const [explain, setExplain] = useState(false);
  const [items, setItems] = useState<HistoryItem[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [searching, setSearching] = useState(false);
  const [query, setQuery] = useState("");
  const [openId, setOpenId] = useState<string | null>(null);
  const count = useRef(PAGE);
  const queryRef = useRef("");
  const [date, setDate] = useState("");
  const dateRef = useRef("");

  const loadStats = useCallback(() => {
    api.getStats().then(setStats).catch(() => {});
  }, []);

  const loadItems = useCallback(() => {
    const text = queryRef.current.trim();
    const limit = count.current;
    const day = dateRef.current;
    let range = {};
    if (day) {
      const [y, m, d] = day.split("-").map(Number);
      const from = new Date(y, m - 1, d).getTime();
      range = { from, to: from + 86_400_000 - 1 };
    }
    api
      .listHistory({ limit: limit + 1, ...(text ? { text } : {}), ...range })
      .then((rows) => {
        setHasMore(rows.length > limit);
        setItems(rows.slice(0, limit));
      })
      .catch(() => {})
      .finally(() => setLoaded(true));
  }, []);

  useEffect(() => {
    loadStats();
    loadItems();
    api
      .keyStatus()
      .then((k) => setKeysMissing(!k.openai || !k.anthropic))
      .catch(() => {});
    const un = events.historyChanged(() => {
      loadStats();
      loadItems();
    });
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  }, [loadStats, loadItems]);

  useEffect(() => {
    queryRef.current = query;
    count.current = PAGE;
    const t = window.setTimeout(loadItems, 250);
    return () => window.clearTimeout(t);
  }, [query, loadItems]);

  const groups = useMemo(() => {
    const out: { label: string; items: HistoryItem[] }[] = [];
    for (const it of items) {
      const label = dayLabel(it.createdAt);
      const last = out[out.length - 1];
      if (last && last.label === label) last.items.push(it);
      else out.push({ label, items: [it] });
    }
    return out;
  }, [items]);

  const opened = items.find((i) => i.id === openId) ?? null;
  const name = settings?.userName?.trim();
  const shortcut = settings ? comboLabel(settings.hotkeys.dictate[0]) : "your shortcut";
  const noHistory = loaded && items.length === 0 && !query.trim();

  return (
    <div className="page page-narrow">
      <div className="page-header">
        <h1 className="page-title">{name ? `Welcome back, ${name}` : "Welcome back"}</h1>
      </div>

      <div className="home-top">
        <section className="hero home-hero">
          <h2>Hold {shortcut} and speak</h2>
          <p>Double-tap for hands-free. Press Esc to cancel.</p>
          {keysMissing ? (
            <button className="btn btn-light" onClick={() => onOpenSettings("providers")}>
              Set up AI keys
            </button>
          ) : (
            <button className="btn btn-light" onClick={() => setExplain((v) => !v)} aria-expanded={explain}>
              How it works
            </button>
          )}
          {explain && !keysMissing && (
            <ol className="home-explain">
              <li>Click into any app where you would normally type.</li>
              <li>Hold {shortcut} and say what you want to write.</li>
              <li>Let go. Your words are cleaned up and typed in for you.</li>
            </ol>
          )}
        </section>

        <section className="card home-stats" aria-label="Your numbers">
          <Stat value={stats ? compactNumber(stats.totalWords) : "0"} label="total words" />
          <Stat value={stats ? String(Math.round(stats.wpm)) : "0"} label="words per minute" />
          <Stat value={stats ? String(stats.streakDays) : "0"} label="day streak" />
        </section>
      </div>

      <div className="home-hist-head">
        <div className="section-label">History</div>
        {searching ? (
          <div className="home-search">
            <SearchBox
              value={query}
              onChange={(v) => {
                setQuery(v);
                if (!v && !date) setSearching(false);
              }}
              placeholder="Search your dictations"
              autoFocus
            />
            <input
              type="date"
              className="input home-date"
              aria-label="Show one day"
              title="Show one day"
              value={date}
              onChange={(e) => {
                setDate(e.target.value);
                dateRef.current = e.target.value;
                count.current = PAGE;
                loadItems();
              }}
            />
          </div>
        ) : (
          <button className="icon-btn" aria-label="Search" onClick={() => setSearching(true)}>
            <Search size={17} />
          </button>
        )}
      </div>

      {noHistory ? (
        <div className="card">
          <EmptyState icon={<Mic size={28} />} title="Nothing here yet">
            Click into any app, hold {shortcut} and say a sentence, then let go. Your words will be typed in and
            kept here.
          </EmptyState>
        </div>
      ) : loaded && items.length === 0 ? (
        <div className="card empty">Nothing matches "{query}".</div>
      ) : (
        groups.map((g) => (
          <section key={g.label} className="home-day">
            <div className="home-day-label">{g.label}</div>
            <div className="list">
              {g.items.map((it) => {
                const st = statusInfo(it);
                const text = itemText(it);
                return (
                  <button key={it.id} className="list-row home-row" onClick={() => setOpenId(it.id)}>
                    <span className="home-time muted">{timeLabel(it.createdAt)}</span>
                    <span className="home-text">
                      {text ? <span className="home-snippet">{text}</span> : null}
                      {st && (
                        <span className={`home-chip ${st.tone}`}>
                          {st.label}
                          {!text && <span className="home-chip-reason"> · {st.reason}</span>}
                        </span>
                      )}
                    </span>
                  </button>
                );
              })}
            </div>
          </section>
        ))
      )}

      {hasMore && (
        <div className="home-more">
          <button
            className="btn btn-soft"
            onClick={() => {
              count.current += PAGE;
              loadItems();
            }}
          >
            Load more
          </button>
        </div>
      )}

      {opened && <HistoryDetail item={opened} onClose={() => setOpenId(null)} onChanged={loadItems} />}
    </div>
  );
}

function Stat({ value, label }: { value: string; label: string }) {
  return (
    <div className="home-stat">
      <div className="display home-stat-num">{value}</div>
      <div className="muted">{label}</div>
    </div>
  );
}
