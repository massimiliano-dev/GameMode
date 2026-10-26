import { useEffect, useRef, useState } from "react";

import { useAction } from "../input/dispatcher";
import { artUrl } from "../lib/api";
import { completionPct, formatHours, formatPlaytime, releaseYear } from "../lib/format";
import { useMetadataRunning } from "../lib/metaStatus";
import type { Game, TimeToBeat } from "../lib/types";

/** Summaries longer than this start folded. */
const FOLD_AT = 420;

const TTB_ROWS: { key: keyof TimeToBeat; label: string }[] = [
  { key: "main", label: "Storia principale" },
  { key: "extra", label: "Storia ed extra" },
  { key: "complete", label: "Completista" },
];

/**
 * The store card below a game's actions: blurb, genres, score, screenshots,
 * time to beat. `onShots` hears how many screenshots can be browsed.
 */
export function GameInfo({ game, onShots }: { game: Game; onShots?: (count: number) => void }) {
  const searching = useMetadataRunning();
  const meta = game.meta;
  if (!meta) {
    return !game.metaCheckedAt && searching ? <p className="setting-hint detail-loading">Cerco la scheda del gioco…</p> : null;
  }

  const year = releaseYear(meta.releaseDate);
  const chips = [...meta.genres, ...(meta.developer ? [meta.developer] : []), ...(year ? [year] : [])];
  const ttb = meta.timeToBeat;
  const hasInfo = meta.summary || chips.length > 0 || meta.criticScore != null;
  // Sections with nothing to press still take focus, or the pad could never
  // scroll down to them; a folded blurb is focusable on its own.
  const infoNav = !meta.summary || meta.summary.length <= FOLD_AT;

  return (
    <>
      {hasInfo && (
        <section className={`detail-section info ${infoNav ? "nav-panel" : ""}`} {...(infoNav ? panelNav : {})}>
          <div className="info-head">
            {meta.criticScore != null && (
              <span className={`score score-${scoreTone(meta.criticScore)}`} role="img" aria-label={`${scoreLabel(meta.scoreSource)} ${meta.criticScore}`}>
                <span className="score-value">{meta.criticScore}</span>
                <span className="score-label">{scoreLabel(meta.scoreSource)}</span>
              </span>
            )}
            {chips.length > 0 && (
              <div className="chips info-chips">
                {chips.map((c) => (
                  <span key={c} className="chip">
                    {c}
                  </span>
                ))}
              </div>
            )}
          </div>
          {meta.summary && <Summary text={meta.summary} />}
        </section>
      )}

      {meta.screenshots.length > 0 && <Screenshots game={game} onCount={onShots} />}

      {ttb && TTB_ROWS.some((r) => ttb[r.key]) && (
        <section className="detail-section nav-panel" {...panelNav}>
          <h2 className="row-title detail-section-title">Tempo per finirlo</h2>
          <dl className="stats ttb">
            {TTB_ROWS.filter((r) => ttb[r.key]).map((r) => (
              <div key={r.key} className="stat">
                <dt>{r.label}</dt>
                <dd>{formatHours(ttb[r.key]!)}</dd>
              </div>
            ))}
          </dl>
          <Progress played={game.playtimeSecs} ttb={ttb} />
        </section>
      )}
    </>
  );
}

const panelNav = { "data-nav": true, tabIndex: -1 } as const;

function scoreLabel(source: "metacritic" | "igdb" | null) {
  return source === "metacritic" ? "Metacritic" : "Voto critica";
}

function scoreTone(score: number) {
  return score >= 75 ? "good" : score >= 50 ? "mid" : "low";
}

/** Long blurbs fold to a few lines; A on the text opens them. */
function Summary({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  if (text.length <= FOLD_AT) return <p className="info-summary">{text}</p>;
  return (
    <button type="button" className={`info-summary info-fold ${open ? "is-open" : ""}`} data-nav onClick={() => setOpen((o) => !o)}>
      <span className="info-text">{text}</span>
      <span className="info-more">{open ? "Mostra meno" : "Leggi tutto"}</span>
    </button>
  );
}

/**
 * One big screenshot with dots. The viewer is a single control: left and
 * right (or LT/RT anywhere on the screen) change the picture.
 */
function Screenshots({ game, onCount }: { game: Game; onCount?: (count: number) => void }) {
  const shots = game.meta?.screenshots ?? [];
  const [index, setIndex] = useState(0);
  const [failed, setFailed] = useState<Set<string>>(() => new Set());
  const ref = useRef<HTMLButtonElement>(null);
  const shown = shots.filter((s) => !failed.has(s));
  const count = shown.length;
  const step = (dir: number) => setIndex((i) => (count ? (i + dir + count) % count : 0));

  useEffect(() => {
    setIndex(0);
    setFailed(new Set());
  }, [game.id]);

  // Only pictures that load count, so the LT/RT hint never promises nothing.
  useEffect(() => {
    onCount?.(count);
    return () => onCount?.(0);
  }, [count, onCount]);

  useEffect(() => {
    const el = ref.current;
    const listener = (e: Event) => step((e as CustomEvent<number>).detail);
    el?.addEventListener("navadjust", listener);
    return () => el?.removeEventListener("navadjust", listener);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [count]);

  useAction((action, repeat) => {
    if (repeat || count < 2 || (action !== "lt" && action !== "rt")) return;
    step(action === "rt" ? 1 : -1);
    return true;
  });

  if (count === 0) return null;
  const current = shown[Math.min(index, count - 1)];
  const src = artUrl(current, game.metaCheckedAt);

  return (
    <section className="detail-section">
      <h2 className="row-title detail-section-title">
        Screenshot
        {count > 1 && (
          <span className="row-count">
            {Math.min(index, count - 1) + 1} di {count}
          </span>
        )}
      </h2>
      <button
        ref={ref}
        type="button"
        className="shots"
        data-nav
        data-nav-capture="x"
        aria-label="Screenshot del gioco, usa sinistra e destra per scorrere"
        onClick={() => step(1)}
      >
        <img
          key={current}
          className="shot"
          src={src}
          alt=""
          decoding="async"
          draggable={false}
          onError={() => setFailed((f) => new Set(f).add(current))}
        />
        {count > 1 && (
          <span className="shot-dots">
            {shown.map((s, i) => (
              <span key={s} className={`shot-dot ${i === Math.min(index, count - 1) ? "is-active" : ""}`} />
            ))}
          </span>
        )}
      </button>
    </section>
  );
}

/** Playtime against the story length, when there is any. */
function Progress({ played, ttb }: { played: number; ttb: TimeToBeat }) {
  const target = ttb.extra ?? ttb.main;
  const label = ttb.extra ? "storia ed extra" : "storia principale";
  const pct = completionPct(played, target);
  if (pct === null || played <= 0) return null;
  return (
    <div className="ttb-progress">
      <span className="meter" role="img" aria-label={`${pct}% della ${label}`}>
        <span className="meter-fill" style={{ width: `${pct}%` }} />
      </span>
      <p className="setting-hint">
        {pct >= 100
          ? `Con ${formatPlaytime(played)} hai superato la durata media della ${label}`
          : `Hai giocato ${formatPlaytime(played)}, circa il ${pct}% della ${label}`}
      </p>
    </div>
  );
}
