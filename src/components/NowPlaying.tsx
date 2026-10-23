import { useEffect, useRef, useState } from "react";

import { Priority, useAction } from "../input/dispatcher";
import { api, on } from "../lib/api";
import type { AppVolume, NowPlaying } from "../lib/types";
import { useApp } from "../store/app";
import { Glyph } from "./common";

const STEP = 0.05;
/** How often the mixer re-reads the audio sessions while it is on screen. */
const POLL_MS = 1000;

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
          if (!stale) setApps(list);
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

  const adjust = (app: AppVolume, dir: number) => {
    const level = Math.min(1, Math.max(0, Math.round((app.level + dir * STEP) * 20) / 20));
    setApps((list) => list.map((a) => (a.key === app.key ? { ...a, level } : a)));
    void api.setAppVolume(app.key, level).catch(() => {});
  };

  const toggleMute = (app: AppVolume) => {
    setApps((list) => list.map((a) => (a.key === app.key ? { ...a, muted: !app.muted } : a)));
    void api.setAppMuted(app.key, !app.muted).catch(() => {});
  };

  return (
    <>
      {track ? (
        <button
          type="button"
          className={`qm-music ${track.playing ? "is-playing" : ""}`}
          data-nav
          onClick={() => control("toggle")}
          title={track.album || undefined}
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
          <button type="button" className="qm-button" data-nav onClick={open}>
            Apri Spotify
            <span className="qm-sub">La musica continua anche nel gioco</span>
          </button>
        )
      )}

      {apps.length > 0 && (
        <div className="qm-mixer" data-nav-group="qm-mixer">
          {apps.map((app) => (
            <MixerRow key={app.key} app={app} onAdjust={(dir) => adjust(app, dir)} onToggleMute={() => toggleMute(app)} />
          ))}
        </div>
      )}
    </>
  );
}

/** One app of the mixer: left/right adjust, A mutes. Same look as the master volume row. */
function MixerRow(props: { app: AppVolume; onAdjust: (dir: number) => void; onToggleMute: () => void }) {
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
      onClick={props.onToggleMute}
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
