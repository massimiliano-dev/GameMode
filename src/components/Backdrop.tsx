import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";

import { artUrl } from "../lib/api";
import { useApp } from "../store/app";

/** Full-screen art of the highlighted game, cross-faded on change. */
export function Backdrop() {
  const game = useApp((s) => s.games.find((g) => g.id === s.backdropId));
  const hero = artUrl(game?.artwork.hero, game?.artworkCheckedAt);
  const cover = artUrl(game?.artwork.cover, game?.artworkCheckedAt);
  const key = game ? `${game.id}:${hero ?? cover ?? "none"}` : "none";

  return (
    <div className="backdrop" aria-hidden>
      <AnimatePresence initial={false}>
        <motion.div
          key={key}
          className="backdrop-layer"
          initial={{ opacity: 0, scale: 1.04 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.6, ease: [0.2, 0.8, 0.2, 1] }}
        >
          {game && <BackdropImage hero={hero} cover={cover} store={game.store} />}
        </motion.div>
      </AnimatePresence>
      <div className="backdrop-shade" />
    </div>
  );
}

function BackdropImage({ hero, cover, store }: { hero?: string; cover?: string; store: string }) {
  const [heroFailed, setHeroFailed] = useState(false);
  if (hero && !heroFailed) {
    return <img className="backdrop-img" src={hero} alt="" onError={() => setHeroFailed(true)} />;
  }
  if (cover) return <img className="backdrop-img is-blurred" src={cover} alt="" />;
  return <div className={`backdrop-img store-glow-${store}`} />;
}
