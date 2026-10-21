import { useEffect, useMemo, useState } from "react";

import { useHints, useScreenFocus } from "../components/common";
import { api } from "../lib/api";
import { SHOP_LABEL } from "../lib/format";
import type { Storefront } from "../lib/types";
import { useApp } from "../store/app";

export function Stores() {
  const games = useApp((s) => s.games);
  const openStore = useApp((s) => s.openStore);
  const setBackdrop = useApp((s) => s.setBackdrop);
  const [shops, setShops] = useState<Storefront[] | null>(null);

  useEffect(() => {
    setBackdrop(null);
    void api.listStorefronts().then(setShops).catch(() => setShops([]));
  }, [setBackdrop]);
  useScreenFocus(undefined, shops !== null);
  useHints([{ button: "A", label: "Apri negozio" }]);

  const owned = useMemo(() => {
    const counts = new Map<string, number>();
    for (const g of games) counts.set(g.store, (counts.get(g.store) ?? 0) + 1);
    return counts;
  }, [games]);

  // Installed clients first: those are the ones you can use with the controller.
  const sorted = useMemo(
    () => [...(shops ?? [])].sort((a, b) => Number(b.clientInstalled) - Number(a.clientInstalled)),
    [shops],
  );

  return (
    <div className="screen stores">
      <header className="stores-head">
        <h1 className="screen-title">Negozi</h1>
        <p className="setting-hint">
          Acquisti e pagamenti avvengono nel client o sul sito ufficiale dello store. Al termine premi Guide per
          tornare qui: la libreria si aggiorna da sola.
        </p>
      </header>
      <div className="stores-grid" data-nav-group="stores">
        {sorted.map((shop, i) => (
          <button
            key={shop.store}
            type="button"
            className={`shop-tile store-${shop.store}`}
            data-nav
            data-nav-autofocus={i === 0 || undefined}
            onClick={() => void openStore(shop.store)}
          >
            <span className="shop-glow" aria-hidden />
            <span className="shop-name">{SHOP_LABEL[shop.store]}</span>
            <span className="shop-meta">
              <span className={`shop-where ${shop.opensIn === "client" ? "is-client" : ""}`}>
                {shop.opensIn === "client" ? "Si apre nel client" : "Si apre nel browser"}
              </span>
              {owned.get(shop.store) ? (
                <span>
                  {owned.get(shop.store)} {owned.get(shop.store) === 1 ? "gioco" : "giochi"} in libreria
                </span>
              ) : null}
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}
