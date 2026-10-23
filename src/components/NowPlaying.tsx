import { type FocusEvent, useEffect, useRef, useState } from "react";

import { Priority, useAction } from "../input/dispatcher";
import { focusElement, focusInitial } from "../input/focus";
import { api, on } from "../lib/api";
import type { AppVolume, NowPlaying } from "../lib/types";
import { useApp } from "../store/app";
import { Glyph } from "./common";

const STEP = 0.05;
/** How often the mixer re-reads the audio sessions while it is on screen. */
const POLL_MS = 1000;
/** After a change is applied, a poll this close may still read the old value. */
const GRACE_MS = 700;
/** `data-nav-id` shared by the track card and the "open Spotify" button, so focus survives the swap. */
const MUSIC_NAV_ID = "qm-music";

type Change = { level?: number; muted?: boolean };
/** One app's changes on their way to the backend: one call in flight, the latest value waiting. */
type Edit = { inflight: boolean; pending: Change | null; settledAt: number };

/**
 * Music in the background of a game: the track Windows' media session
 * plays (A = play/pause, LB/RB = previous/next), a shortcut to open
 * Spotify, and a mixer with one volume per app (game, music, chat…) under
 * the master volume. Used by the in-game overlay and the quick menu.
 */
export function MusicPanel(props: {
  /** Polls the mixer and takes LB/RB only while true (the overlay stays mounted when hidden). */
  active: boolean;
  /** Offers to open Spotify when nothing is playing. */
  showOpen?: boolean;
}) {
  const enabled = useApp((s) => s.settings?.musicControls ?? true);
  const toast = useApp((s) => s.toast);
  const [track, setTrack] = useState<NowPlaying | null>(null);
  const [apps, setApps] = useState<AppVolume[]>([]);
  const appsRef = useRef(apps);
  appsRef.current = apps;
  // Changes per app key, applied one at a time and coalesced (holding a
  // direction fires every 80 ms; only the latest level matters).
  const edits = useRef(new Map<string, Edit>());
  // What the controller is on inside this panel: "music" or a mixer key.
  // Elements can vanish under the focus (a row's session ends, the open
  // button gives way to the track card); then focus is put back nearby.
  const focused = useRef<string | null>(null);
  const active = enabled && props.active;

  useEffect(() => on<NowPlaying | null>("media://now-playing", setTrack), []);

  useEffect(() => {
    if (!active) return;
    void api.mediaNowPlaying().then(setTrack).catch(() => setTrack(null));
    let stale = false;
    const poll = () => {
      void api
        .appVolumes()
        .then((list) => {
          if (stale) return;
          const now = Date.now();
          const previous = appsRef.current;
          // Keep what the user just set: the backend may not have applied it yet.
          const merged = list.map((a) => {
            const edit = edits.current.get(a.key);
            const recent = edit && (edit.inflight || edit.pending !== null || now - edit.settledAt < GRACE_MS);
            const mine = recent ? previous.find((p) => p.key === a.key) : undefined;
            return mine ? { ...a, level: mine.level, muted: mine.muted } : a;
          });
          const lost = focused.current !== null && focused.current !== "music" && !merged.some((a) => a.key === focused.current);
          const neighbour = lost ? merged[Math.min(Math.max(0, previous.findIndex((a) => a.key === focused.current)), merged.length - 1)] : undefined;
          setApps(merged);
          if (lost) requestAnimationFrame(() => recoverFocus(neighbour?.key));
        })
        .catch(() => {});
    };
    poll();
    const timer = setInterval(poll, POLL_MS);
    return () => {
      stale = true;
      clearInterval(timer);
    };
  }, [active]);

  // The open button and the track card replace each other.
  useEffect(() => {
    if (focused.current === "music") requestAnimationFrame(() => recoverFocus());
  }, [track]);

  useAction(
    (action, repeat) => {
      if (repeat || !track) return;
      if (action === "lb" || action === "rb") {
        void api.mediaControl(action === "lb" ? "prev" : "next").catch((e) => toast(String(e), "error"));
        return true;
      }
    },
    Priority.MODAL + 1,
    active,
  );

  if (!enabled) return null;

  const control = (action: "toggle") => {
    if (!track) return;
    setTrack({ ...track, playing: !track.playing });
    void api.mediaControl(action).catch((e) => toast(String(e), "error"));
  };

  const open = () => void api.openMusic().catch((e) => toast(String(e), "error"));

  /** Sends the pending change of `key` when nothing else is in flight for it. */
  const flush = (key: string) => {
    const edit = edits.current.get(key);
    if (!edit || edit.inflight || !edit.pending) return;
    const { level, muted } = edit.pending;
    edit.pending = null;
    edit.inflight = true;
    const calls: Promise<void>[] = [];
    if (level !== undefined) calls.push(api.setAppVolume(key, level));
    if (muted !== undefined) calls.push(api.setAppMuted(key, muted));
    void Promise.allSettled(calls).then(() => {
      edit.inflight = false;
      edit.settledAt = Date.now();
      flush(key);
    });
  };

  const queue = (key: string, change: Change) => {
    const edit = edits.current.get(key) ?? { inflight: false, pending: null, settledAt: 0 };
    edit.pending = { ...edit.pending, ...change };
    edits.current.set(key, edit);
    flush(key);
  };

  const adjust = (app: AppVolume, dir: number) => {
    const level = Math.min(1, Math.max(0, Math.round((app.level + dir * STEP) * 20) / 20));
    setApps((list) => list.map((a) => (a.key === app.key ? { ...a, level } : a)));
    queue(app.key, { level });
  };

  const toggleMute = (app: AppVolume) => {
    setApps((list) => list.map((a) => (a.key === app.key ? { ...a, muted: !app.muted } : a)));
    queue(app.key, { muted: !app.muted });
  };

  const focusProps = (what: string) => ({
    onFocus: () => {
      focused.current = what;
    },
    // A blur with no target is the element going away (or the window):
    // the choice is kept so it can be restored.
    onBlur: (e: FocusEvent) => {
      if (e.relatedTarget) focused.current = null;
    },
  });

  return (
    <>
      {track ? (
        <button
          type="button"
          className={`qm-music ${track.playing ? "is-playing" : ""}`}
          data-nav
          data-nav-id={MUSIC_NAV_ID}
          onClick={() => control("toggle")}
          title={track.album || undefined}
          {...focusProps("music")}
        >
          <span className="qm-music-cover">{track.thumbnail ? <img src={track.thumbnail} alt="" draggable={false} /> : "♪"}</span>
          <span className="qm-music-info">
            <span className="qm-music-title">{track.title || "Senza titolo"}</span>
            <span className="qm-music-artist">{[track.artist, track.source].filter(Boolean).join(" · ")}</span>
            <span className="qm-music-state">
              {track.playing ? "In riproduzione" : "In pausa"} · <Glyph button="A" /> {track.playing ? "pausa" : "riproduci"} · <Glyph button="LB" />
              <Glyph button="RB" /> brano
            </span>
          </span>
        </button>
      ) : (
        props.showOpen && (
          <button type="button" className="qm-button" data-nav data-nav-id={MUSIC_NAV_ID} onClick={open} {...focusProps("music")}>
            Apri Spotify
            <span className="qm-sub">La musica continua anche nel gioco</span>
          </button>
        )
      )}

      {apps.length > 0 && (
        <div className="qm-mixer" data-nav-group="qm-mixer">
          {apps.map((app) => (
            <MixerRow key={app.key} app={app} onAdjust={(dir) => adjust(app, dir)} onToggleMute={() => toggleMute(app)} {...focusProps(app.key)} />
          ))}
        </div>
      )}
    </>
  );
}

/** Puts the focus back when the element under it was removed: on the mixer row `key`, else on the music card. */
function recoverFocus(key?: string) {
  if (document.activeElement !== document.body) return;
  const row = key ? document.querySelector<HTMLElement>(`[data-mixer-key="${CSS.escape(key)}"]`) : null;
  if (row) focusElement(row);
  else focusInitial(MUSIC_NAV_ID);
}

/** One app of the mixer: left/right adjust, A mutes. Same look as the master volume row. */
function MixerRow(props: {
  app: AppVolume;
  onAdjust: (dir: number) => void;
  onToggleMute: () => void;
  onFocus: () => void;
  onBlur: (e: FocusEvent) => void;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const adjust = useRef(props.onAdjust);
  adjust.current = props.onAdjust;

  useEffect(() => {
    const el = ref.current;
    const listener = (e: Event) => adjust.current((e as CustomEvent<number>).detail);
    el?.addEventListener("navadjust", listener);
    return () => el?.removeEventListener("navadjust", listener);
  }, []);

  const { app } = props;
  const pct = Math.round(app.level * 100);
  return (
    <button
      ref={ref}
      type="button"
      className={`qm-volume is-app ${app.muted ? "is-muted" : ""} ${app.active ? "" : "is-idle"}`}
      data-nav
      data-nav-capture="x"
      data-mixer-key={app.key}
      onClick={props.onToggleMute}
      onFocus={props.onFocus}
      onBlur={props.onBlur}
    >
      <span className="qm-label">
        {app.label} <span className="qm-sub">{app.muted ? "muto · A per riattivare" : app.active ? "" : "in silenzio"}</span>
      </span>
      <span className="qm-volume-bar">
        <span className="qm-volume-fill" style={{ width: `${app.muted ? 0 : pct}%` }} />
      </span>
      <span className="qm-volume-value">{app.muted ? "Muto" : `${pct}%`}</span>
    </button>
  );
}
