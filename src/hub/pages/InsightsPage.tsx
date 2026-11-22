import { BarChart3 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { api, compactNumber, events, type Stats } from "../../shared/api";
import { EmptyState } from "../components";
import { useSettings } from "../useSettings";
import { buildHeatmap, formatMinutes, heatBand, weeklyTotals } from "./insights/utils";
import "./insights.css";

function Gauge({ wpm }: { wpm: number }) {
  // Semicircle gauge, 0..200 wpm.
  const max = 200;
  const frac = Math.min(1, Math.max(0, wpm / max));
  const r = 52;
  const len = Math.PI * r;
  return (
    <svg viewBox="0 0 130 74" className="gauge" role="img" aria-label={`${Math.round(wpm)} words per minute`}>
      <path d="M13 65 A52 52 0 0 1 117 65" className="gauge-track" />
      <path
        d="M13 65 A52 52 0 0 1 117 65"
        className="gauge-fill"
        strokeDasharray={`${len * frac} ${len}`}
      />
    </svg>
  );
}

function Tile({ label, children, note }: { label: string; children: React.ReactNode; note?: string }) {
  return (
    <div className="card ins-tile">
      <div className="section-label">{label}</div>
      <div className="ins-tile-body">{children}</div>
      {note && <div className="muted ins-note">{note}</div>}
    </div>
  );
}

function TrendChart({ daily }: { daily: Stats["daily"] }) {
  const weeks = useMemo(() => weeklyTotals(daily, 12), [daily]);
  const max = Math.max(1, ...weeks.map((w) => w.words));
  const W = 560;
  const H = 150;
  const slot = W / weeks.length;
  const bw = slot * 0.6;
  return (
    <svg viewBox={`0 0 ${W} ${H + 24}`} className="trend" role="img" aria-label="Words dictated per week over the last 12 weeks">
      <line x1="0" x2={W} y1={H} y2={H} className="trend-axis" />
      {weeks.map((w, i) => {
        const h = Math.max(w.words > 0 ? 3 : 0, (w.words / max) * (H - 10));
        const x = i * slot + (slot - bw) / 2;
        const label = new Date(w.start + "T00:00").toLocaleDateString("en-GB", { day: "numeric", month: "short" });
        return (
          <g key={w.start}>
            <rect x={x} y={H - h} width={bw} height={h} rx="4" className={i === weeks.length - 1 ? "trend-bar now" : "trend-bar"}>
              <title>{`Week of ${label}: ${w.words.toLocaleString("en-GB")} words`}</title>
            </rect>
            {(i % 2 === 1 || i === weeks.length - 1) && (
              <text x={x + bw / 2} y={H + 16} textAnchor="middle" className="trend-label">
                {label}
              </text>
            )}
          </g>
        );
      })}
    </svg>
  );
}

function Heatmap({ daily }: { daily: Stats["daily"] }) {
  const { columns, months } = useMemo(() => buildHeatmap(daily, 26), [daily]);
  const rows = ["", "Mon", "", "Wed", "", "Fri", ""];
  return (
    <div className="heat" role="img" aria-label="Daily dictation activity over the last 26 weeks">
      <div className="heat-rows">
        <div className="heat-month-spacer" />
        {rows.map((r, i) => (
          <div key={i} className="heat-row-label">
            {r}
          </div>
        ))}
      </div>
      <div className="heat-grid">
        <div className="heat-months" style={{ gridTemplateColumns: `repeat(${columns.length}, 1fr)` }}>
          {months.map((m) => (
            <span key={m.col} style={{ gridColumn: m.col + 1 }}>
              {m.label}
            </span>
          ))}
        </div>
        <div className="heat-cols" style={{ gridTemplateColumns: `repeat(${columns.length}, 1fr)` }}>
          {columns.map((col, c) => (
            <div key={c} className="heat-col">
              {col.map((cell, r) =>
                cell ? (
                  <div
                    key={r}
                    className={`heat-cell band-${heatBand(cell.words)}`}
                    title={`${new Date(cell.date + "T00:00").toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short" })}: ${cell.words.toLocaleString("en-GB")} words`}
                  />
                ) : (
                  <div key={r} className="heat-cell empty" />
                ),
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

export default function InsightsPage() {
  const { settings } = useSettings();
  const [stats, setStats] = useState<Stats | null>(null);
  const [loaded, setLoaded] = useState(false);

  const load = useCallback(() => {
    api
      .getStats()
      .then(setStats)
      .catch(() => {})
      .finally(() => setLoaded(true));
  }, []);

  useEffect(() => {
    load();
    const un = events.historyChanged(load);
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  }, [load]);

  const typingWpm = settings?.typingWpm ?? 40;
  const empty = !stats || stats.totalWords === 0;
  const topMax = Math.max(1, ...(stats?.topApps ?? []).map((a) => a.words));

  return (
    <div className="page page-narrow">
      <div className="page-header">
        <h1 className="page-title">Your usage</h1>
      </div>

      {!loaded ? null : empty ? (
        <div className="card">
          <EmptyState icon={<BarChart3 size={28} />} title="No dictations yet">
            Once you have dictated a few things, your speed, time saved and streak will show up here.
          </EmptyState>
        </div>
      ) : (
        <>
          <div className="ins-grid">
            <Tile label="Speed">
              <div className="gauge-wrap">
                <Gauge wpm={stats.wpm} />
                <div className="display ins-big gauge-num">{Math.round(stats.wpm)}</div>
              </div>
              <div className="muted ins-note">words per minute</div>
            </Tile>
            <Tile label="Total words dictated">
              <div className="display ins-big">{compactNumber(stats.totalWords)}</div>
              <div className="muted ins-note">{stats.totalDictations.toLocaleString("en-GB")} dictations</div>
            </Tile>
            <Tile label="Words this week">
              <div className="display ins-big">{compactNumber(stats.wordsThisWeek)}</div>
              <div className="muted ins-note">{stats.wordsToday.toLocaleString("en-GB")} today</div>
            </Tile>
            <Tile label="Time saved">
              <div className="display ins-big">{formatMinutes(stats.minutesSaved)}</div>
              <div className="muted ins-note">{formatMinutes(stats.minutesSavedThisWeek)} this week</div>
            </Tile>
            <Tile label="Fixes made">
              <div className="display ins-big">{compactNumber(stats.fixes)}</div>
              <div className="muted ins-note">spellings and words corrected for you</div>
            </Tile>
          </div>
          <p className="muted ins-foot">Time saved compares with typing at {typingWpm} words per minute.</p>

          <section className="card ins-section">
            <div className="section-label">Words per week, last 12 weeks</div>
            <TrendChart daily={stats.daily} />
          </section>

          <section className="card ins-section">
            <div className="ins-streak-head">
              <div>
                <div className="display ins-streak">{stats.streakDays} day streak</div>
                <div className="muted">Longest streak | {stats.longestStreak} days</div>
              </div>
              <div className="heat-legend" aria-hidden>
                <span className="muted">Less</span>
                {[0, 1, 2, 3, 4].map((b) => (
                  <span key={b} className={`heat-cell band-${b}`} />
                ))}
                <span className="muted">More</span>
              </div>
            </div>
            <Heatmap daily={stats.daily} />
          </section>

          <section className="card ins-section">
            <div className="section-label">Top apps</div>
            {stats.topApps.length === 0 ? (
              <p className="muted">Nothing here yet.</p>
            ) : (
              <ul className="apps">
                {stats.topApps.slice(0, 8).map((a) => (
                  <li key={a.name}>
                    <span className="app-name">{a.name}</span>
                    <span className="app-bar-wrap">
                      <span className="app-bar" style={{ width: `${Math.max(3, (a.words / topMax) * 100)}%` }} />
                    </span>
                    <span className="muted app-words">{a.words.toLocaleString("en-GB")} words</span>
                  </li>
                ))}
              </ul>
            )}
          </section>
        </>
      )}
    </div>
  );
}
