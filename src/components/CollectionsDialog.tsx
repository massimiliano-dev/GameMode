import { motion } from "motion/react";
import { useState } from "react";

import { Priority, useAction } from "../input/dispatcher";
import type { Game } from "../lib/types";
import { useApp } from "../store/app";
import { Toggle, useScreenFocus } from "./common";

/** Puts a game in or out of the active profile's collections. */
export function CollectionsDialog({ game, onClose }: { game: Game; onClose: () => void }) {
  const collections = useApp((s) => s.collections);
  const setInCollection = useApp((s) => s.setInCollection);
  const createCollection = useApp((s) => s.createCollection);
  const [name, setName] = useState("");

  useScreenFocus();
  useAction((action) => {
    if (action === "back") {
      onClose();
      return true;
    }
    return !["up", "down", "left", "right", "confirm"].includes(action);
  }, Priority.BLOCKING);

  const create = async () => {
    if (!name.trim()) return;
    const c = await createCollection(name);
    if (c) {
      await setInCollection(c.id, game.id, true);
      setName("");
    }
  };

  return (
    <motion.div
      className="dialog-scrim"
      data-nav-layer="collections"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.18 }}
    >
      <motion.div className="dialog" role="dialog" aria-labelledby="collections-title" initial={{ scale: 0.94 }} animate={{ scale: 1 }}>
        <h2 id="collections-title" className="dialog-title">
          Raccolte
        </h2>
        <p className="dialog-text">Scegli dove mettere {game.name}.</p>
        <div className="settings-list dialog-list">
          {collections.map((c, i) => {
            const inside = c.gameIds.includes(game.id);
            return (
              <button
                key={c.id}
                type="button"
                className="setting-row"
                data-nav
                data-nav-autofocus={i === 0 || undefined}
                onClick={() => void setInCollection(c.id, game.id, !inside)}
              >
                <span className="setting-text">
                  <span className="setting-label">{c.name}</span>
                  <span className="setting-hint">
                    {c.gameIds.length} {c.gameIds.length === 1 ? "gioco" : "giochi"}
                  </span>
                </span>
                <Toggle on={inside} />
              </button>
            );
          })}
          <div className="setting-inline dialog-new">
            <input
              className="text-input"
              data-nav
              data-nav-autofocus={collections.length === 0 || undefined}
              aria-label="Nome della nuova raccolta"
              placeholder="Nuova raccolta…"
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && void create()}
            />
            <button type="button" className="btn" data-nav onClick={() => void create()}>
              Crea
            </button>
          </div>
        </div>
        <div className="button-row dialog-actions">
          <button type="button" className="btn btn-primary" data-nav onClick={onClose}>
            Fatto
          </button>
        </div>
      </motion.div>
    </motion.div>
  );
}
