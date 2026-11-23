import { Mic, Search, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, compactNumber, comboLabel, events, type HistoryItem, type Stats } from "../../shared/api";
import { shouldOfferSignIn } from "../../shared/account";
import { useAccountStatus } from "../useAccount";
import { EmptyState, SearchBox, dayLabel } from "../components";
import { useSettings } from "../useSettings";
import HistoryDetail from "./home/HistoryDetail";
import HistoryRow from "./home/HistoryRow";
import WaitingRow from "./home/WaitingRow";
import { keysMissing as needKeys, type KeyState } from "./home/helpers";
import "./home.css";

const PAGE = 50;
const DISMISS_KEY = "wisper.accountCardDismissed";

function readDismissed(): boolean {
  try {
    return localStorage.getItem(DISMISS_KEY) === "1";
  } catch {
    return false;
  }
}

export default function HomePage({ onOpenSettings }: { onOpenSettings: (section?: string) => void }) {
  const { settings } = useSettings();
  const { status: account } = useAccountStatus();
  const [cardDismissed, setCardDismissed] = useState(readDismissed);
  const [stats, setStats] = useState<Stats | null>(null);
  const [keys, setKeys] = useState<KeyState | null>(null);
  const [silent, setSilent] = useState<HistoryItem[]>([]);
  const [showSilent, setShowSilent] = useState(false);
  const [waiting, setWaiting] = useState<HistoryItem[]>([]);
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
    // Without a search or a day filter, recordings still waiting to be processed are shown
    // as one row at the top instead of one row each.
    const grouped = !text && !day;
    api
      .listHistory({ limit: limit + 1, ...(text ? { text } : {}), ...range, ...(grouped ? { excludeStatus: ["queued", "failed"] } : {}) })
      .then((rows) => {
        setHasMore(rows.length > limit);
        setItems(rows.slice(0, limit));
      })
      .catch(() => {})
      .finally(() => setLoaded(true));
    if (grouped) {
      Promise.all([api.listHistory({ status: "queued", limit: 500 }), api.listHistory({ status: "failed", limit: 500 })])
        .then(([a, b]) => setWaiting([...a, ...b].sort((x, y) => y.createdAt - x.createdAt)))
        .catch(() => {});
      api
        .listHistory({ status: "silent", limit: 200 })
        .then(setSilent)
        .catch(() => {});
    } else {
      setWaiting([]);
    }
  }, []);

  const loadKeys = useCallback(() => {
    api
      .keyStatus()
      .then(setKeys)
      .catch(() => {});
  }, []);

  useEffect(() => {
    loadStats();
    loadItems();
    loadKeys();
    const unsubs = [
      events.historyChanged(() => {
        loadStats();
        loadItems();
      }),
      // Keys saved or removed in Settings, or a different AI chosen: re-check what is missing.
      events.keysChanged(loadKeys),
      events.settingsChanged(loadKeys),
    ];
    return () => {
      unsubs.forEach((u) => u.then((f) => f()).catch(() => {}));
    };
  }, [loadStats, loadItems, loadKeys]);

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

  const opened = items.find((i) => i.id === openId) ?? silent.find((i) => i.id === openId) ?? waiting.find((i) => i.id === openId) ?? null;
  const name = settings?.userName?.trim();
  const shortcut = settings ? comboLabel(settings.hotkeys.dictate[0]) : "your shortcut";
  const keysMissing = needKeys(keys, settings?.providers.cleanupProvider);
  const noHistory = loaded && items.length === 0 && waiting.length === 0 && !query.trim() && !date;

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

      {shouldOfferSignIn(account, cardDismissed) && (
        <section className="card home-account-card" aria-label="Sync your computers">
          <p>Use Wisper on more than one computer? Sign in to sync your keys and settings.</p>
          <button className="btn btn-soft" onClick={() => onOpenSettings("account")}>
            Sign in…
          </button>
          <button
            className="icon-btn"
            aria-label="Dismiss"
            title="Dismiss"
            onClick={() => {
              setCardDismissed(true);
              try {
                localStorage.setItem(DISMISS_KEY, "1");
              } catch {
                /* private window: just hide it for now */
              }
            }}
          >
            <X size={16} />
          </button>
        </section>
      )}

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

      <WaitingRow items={waiting} keys={keys} onOpen={setOpenId} onChanged={loadItems} />

      {noHistory ? (
        <div className="card">
          <EmptyState icon={<Mic size={28} />} title="Nothing here yet">
            Click into any app, hold {shortcut} and say a sentence, then let go. Your words will be typed in and
            kept here.
          </EmptyState>
        </div>
      ) : loaded && items.length === 0 && waiting.length === 0 && (query.trim() || date) ? (
        <div className="card empty">Nothing matches{query.trim() ? ` "${query}"` : " that day"}.</div>
      ) : (
        groups.map((g) => (
          <section key={g.label} className="home-day">
            <div className="home-day-label">{g.label}</div>
            <div className="list">
              {g.items.map((it) => (
                <HistoryRow key={it.id} item={it} onOpen={() => setOpenId(it.id)} />
              ))}
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

      {silent.length > 0 && !query.trim() && !date && (
        <div className="home-silent">
          <button className="link-btn muted" onClick={() => setShowSilent((v) => !v)} aria-expanded={showSilent}>
            {showSilent ? "Hide" : "Show"} recordings where no speech was heard (last 24 hours)
          </button>
          {showSilent && (
            <div className="list home-silent-list">
              {silent.map((it) => (
                <HistoryRow key={it.id} item={it} onOpen={() => setOpenId(it.id)} />
              ))}
            </div>
          )}
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
