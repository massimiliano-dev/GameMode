import { useEffect, useState } from "react";

import { api } from "../lib/api";
import { formatLastPlayed } from "../lib/format";
import type { SteamExtras as Extras } from "../lib/types";

const SHOWN_ACHIEVEMENTS = 18;

/** Achievements and official news for a Steam game, below its detail. */
export function SteamExtras({ appid }: { appid: number }) {
  const [data, setData] = useState<Extras | null>(null);

  useEffect(() => {
    let alive = true;
    setData(null);
    void api.steamExtras(appid).then((d) => alive && setData(d)).catch(() => {});
    return () => {
      alive = false;
    };
  }, [appid]);

  if (!data) return <p className="setting-hint detail-loading">Carico obiettivi e notizie…</p>;

  const unlocked = data.achievements.filter((a) => a.achieved).length;
  const total = data.achievements.length;

  return (
    <>
      {data.achievementStatus !== "none" && (
        <section className="detail-section">
          <h2 className="row-title detail-section-title">
            Obiettivi
            {data.achievementStatus === "ok" && (
              <span className="row-count">
                {unlocked} di {total}
              </span>
            )}
          </h2>
          {data.achievementStatus === "ok" ? (
            <>
              <span className="meter" role="img" aria-label={`${unlocked} obiettivi su ${total}`}>
                <span className="meter-fill" style={{ width: `${(unlocked / total) * 100}%` }} />
              </span>
              <div className="achievements" data-nav-group="achievements">
                {data.achievements.slice(0, SHOWN_ACHIEVEMENTS).map((a, i) => (
                  <button
                    key={`${a.name}-${i}`}
                    type="button"
                    className={`achievement ${a.achieved ? "is-unlocked" : ""}`}
                    data-nav
                  >
                    {a.icon ? <img src={a.icon} alt="" loading="lazy" /> : <span className="achievement-icon">{a.achieved ? "★" : "☆"}</span>}
                    <span className="achievement-text">
                      <span className="achievement-name">{a.name}</span>
                      <span className="achievement-desc">
                        {a.achieved ? `Sbloccato ${formatLastPlayed(a.unlockTime)}` : a.description || "Da sbloccare"}
                      </span>
                    </span>
                  </button>
                ))}
              </div>
            </>
          ) : (
            <p className="setting-hint">
              {data.achievementStatus === "needsKey"
                ? "Inserisci la chiave Web API di Steam nelle Impostazioni per vedere i tuoi obiettivi."
                : "Obiettivi non disponibili: nel profilo Steam i dettagli dei giochi devono essere visibili."}
            </p>
          )}
        </section>
      )}

      {data.news.length > 0 && (
        <section className="detail-section">
          <h2 className="row-title detail-section-title">Novità</h2>
          <div className="news-list">
            {data.news.map((n) => (
              <button key={n.url + n.date} type="button" className="news-item" data-nav onClick={() => void api.openLink(n.url)}>
                <span className="news-date">{formatLastPlayed(n.date)}</span>
                <span className="news-title">{n.title}</span>
                <span className="news-snippet">{n.snippet}</span>
              </button>
            ))}
          </div>
        </section>
      )}
    </>
  );
}
