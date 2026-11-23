import { AlertTriangle, ClipboardCopy, Mic, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, compactNumber, comboLabel, events, type HistoryItem, type Stats } from "../../shared/api";
import { shouldOfferSignIn } from "../../shared/account";
import { useAccountStatus } from "../useAccount";
import { EmptyState, SearchBox, dayLabel, errorText, timeLabel, useToast } from "../components";
import { useSettings } from "../useSettings";
import HistoryDetail from "./home/HistoryDetail";
import { itemText, statusInfo } from "./home/helpers";
import "./home.css";

const PAGE = 50;

function greeting(now = new Date()): string {
  const h = now.getHours();
  return h < 5 ? "Good evening" : h < 12 ? "Good morning" : h < 18 ? "Good afternoon" : "Good evening";
}
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
  const [keysMissing, setKeysMissing] = useState(false);
  const [explain, setExplain] = useState(false);
  const toast = useToast();
  const [items, setItems] = useState<HistoryItem[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [loaded, setLoaded] = useState(false);
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

  const keys = settings ? comboLabel(settings.hotkeys.dictate[0]).split(" + ") : [];
  const copy = (text: string) =>
    api
      .copyText(text)
      .then(() => toast("Copied"))
      .catch((e) => toast(errorText(e), "error"));

  return (
    <div className="page page-narrow">
      <div className="page-header">
        <h1 className="page-title">{name ? `${greeting()}, ${name}` : greeting()}</h1>
      </div>

      <section className="talk" aria-labelledby="talk-title">
        {keysMissing && (
          <div className="talk-warn" role="alert">
            <AlertTriangle size={17} aria-hidden />
            <span>Wisper can’t turn speech into text until your AI keys are added.</span>
            <button className="btn btn-dark btn-sm" onClick={() => onOpenSettings("providers")}>
              Add AI keys
            </button>
          </div>
        )}
        <div className="talk-main">
          <div className="talk-how">
            <h2 id="talk-title" className="talk-title">
              To dictate, hold
            </h2>
            <div className="talk-keys" aria-label={settings ? shortcut : "your shortcut"}>
              {keys.length ? (
                keys.map((k, i) => (
                  <span key={k} className="talk-keywrap">
                    {i > 0 && <span className="talk-plus" aria-hidden>+</span>}
                    <kbd className="talk-key">{k}</kbd>
                  </span>
                ))
              ) : (
                <kbd className="talk-key">…</kbd>
              )}
            </div>
            <p className="talk-sub">Speak, then let go. Your words are typed wherever your cursor is.</p>
          </div>
          <ul className="talk-tips">
            <li>
              <span>Hands-free</span>
              <span className="muted">Double-tap the shortcut, tap again to finish</span>
            </li>
            <li>
              <span>Cancel</span>
              <span className="muted">
                Press <kbd>Esc</kbd> while recording
              </span>
            </li>
            <li>
              <button className="link-btn" onClick={() => setExplain((v) => !v)} aria-expanded={explain}>
                {explain ? "Hide the steps" : "Show me step by step"}
              </button>
            </li>
          </ul>
        </div>
        {explain && (
          <ol className="talk-steps">
            <li>Click into any app where you would normally type.</li>
            <li>Hold {shortcut} and say what you want to write.</li>
            <li>Let go. Your words are cleaned up and typed in for you.</li>
          </ol>
        )}
        <dl className="talk-stats" aria-label="Your numbers">
          <Stat value={stats ? compactNumber(stats.totalWords) : "0"} label="words dictated" />
          <Stat value={stats ? String(Math.round(stats.wpm)) : "0"} label="words per minute" />
          <Stat value={stats ? String(stats.streakDays) : "0"} label={stats?.streakDays === 1 ? "day in a row" : "days in a row"} />
        </dl>
      </section>

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
        <h2 className="section-label">What you said</h2>
        <div className="home-search">
          <SearchBox value={query} onChange={setQuery} placeholder="Search your dictations" />
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
      </div>

      {noHistory && !date ? (
        <div className="card">
          <EmptyState icon={<Mic size={28} />} title="Nothing here yet">
            Click into any app, hold {shortcut} and say a sentence, then let go. Your words will be typed in and
            kept here.
          </EmptyState>
        </div>
      ) : loaded && items.length === 0 ? (
        <div className="card empty">
          {query.trim() ? `Nothing matches “${query.trim()}”.` : "Nothing on that day."}
        </div>
      ) : (
        groups.map((g) => (
          <section key={g.label} className="home-day" aria-label={g.label}>
            <h3 className="home-day-label">{g.label}</h3>
            <div className="list">
              {g.items.map((it) => {
                const st = statusInfo(it);
                const text = itemText(it);
                return (
                  <div key={it.id} className="list-row home-row">
                    <button className="home-open" onClick={() => setOpenId(it.id)} aria-label={`Open dictation from ${timeLabel(it.createdAt)}`}>
                      <span className="home-time muted">{timeLabel(it.createdAt)}</span>
                      <span className="home-text">
                        {text ? <span className="home-snippet">{text}</span> : null}
                        {st && (
                          <span className={`home-chip ${st.tone}`}>
                            {st.label}
                            {!text && <span className="home-chip-reason">: {st.reason}</span>}
                          </span>
                        )}
                      </span>
                    </button>
                    {text && (
                      <button className="icon-btn home-copy" onClick={() => copy(text)} aria-label="Copy this text" title="Copy">
                        <ClipboardCopy size={16} />
                      </button>
                    )}
                  </div>
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
      <dt className="muted">{label}</dt>
      <dd className="display home-stat-num">{value}</dd>
    </div>
  );
}
