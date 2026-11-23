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

      {keysMissing && (
        <div className="talk-warn" role="alert">
          <AlertTriangle size={17} aria-hidden />
          <span>Wisper can’t turn speech into text until your AI keys are added.</span>
          <button className="btn btn-dark btn-sm" onClick={() => onOpenSettings("providers")}>
            Add AI keys
          </button>
        </div>
      )}

      <section className="talk" aria-labelledby="talk-title">
        <div className="talk-how">
          <h2 id="talk-title" className="talk-title">
            Hold to dictate
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
        </div>
        <ul className="talk-tips">
          <li>Speak, then let go. Your words appear wherever your cursor is.</li>
          <li>
            <strong>Double-tap</strong> to keep going hands-free. <kbd>Esc</kbd> cancels.
          </li>
          <li>
            <button className="link-btn" onClick={() => setExplain((v) => !v)} aria-expanded={explain}>
              {explain ? "Hide the steps" : "Show me step by step"}
            </button>
          </li>
        </ul>
        <dl className="talk-stats" aria-label="Your numbers">
          <Stat value={stats ? compactNumber(stats.totalWords) : "0"} label="words" />
          <Stat value={stats ? String(Math.round(stats.wpm)) : "0"} label="per minute" />
          <Stat value={stats ? String(stats.streakDays) : "0"} label={stats?.streakDays === 1 ? "day running" : "days running"} />
        </dl>
        {explain && (
          <ol className="talk-steps">
            <li>Click into any app where you would normally type.</li>
            <li>Hold {shortcut} and say what you want to write.</li>
            <li>Let go. Your words are cleaned up and typed in for you.</li>
          </ol>
        )}
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
        <div className="hist" role="list" aria-label="Your dictations">
          {groups.map((g) => (
            <div key={g.label} role="presentation">
              <h3 className="hist-day">{g.label}</h3>
              {g.items.map((it) => {
                const st = statusInfo(it);
                const text = itemText(it);
                return (
                  <div key={it.id} className="hist-row" role="listitem">
                    <button className="hist-open" onClick={() => setOpenId(it.id)} aria-label={`Open dictation from ${timeLabel(it.createdAt)}`}>
                      <span className="hist-time">{timeLabel(it.createdAt)}</span>
                      <span className="hist-text">
                        {text ? <span className="home-snippet">{text}</span> : null}
                        {st && (
                          <span className={`home-chip ${st.tone}`}>
                            {st.label}
                            {!text && <span className="home-chip-reason">: {st.reason}</span>}
                          </span>
                        )}
                      </span>
                      <span className="hist-app">{it.appName ?? ""}</span>
                    </button>
                    {text ? (
                      <button className="icon-btn hist-copy" onClick={() => copy(text)} aria-label="Copy this text" title="Copy">
                        <ClipboardCopy size={16} />
                      </button>
                    ) : (
                      <span className="hist-copy-space" />
                    )}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
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
