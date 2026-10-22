import { AnimatePresence } from "motion/react";
import { useState } from "react";

import { ensureFocus } from "../input/focus";
import { api } from "../lib/api";
import type { Collection, Profile } from "../lib/types";
import { useApp } from "../store/app";
import { ActionSheet } from "./ActionSheet";
import { ConfirmDialog } from "./ConfirmDialog";
import { Row, Section } from "./SettingsRows";

type Pending =
  | { kind: "profile"; profile: Profile }
  | { kind: "deleteProfile"; profile: Profile }
  | { kind: "deleteCollection"; collection: Collection }
  | null;

export function Avatar({ profile, size = "md" }: { profile?: Profile; size?: "sm" | "md" }) {
  if (!profile) return null;
  return (
    <span className={`avatar avatar-${size}`} style={{ background: profile.color }} aria-hidden>
      {profile.name.trim().charAt(0).toUpperCase() || "?"}
    </span>
  );
}

/** Settings sections for household profiles and the active profile's collections. */
export function ProfileSettings() {
  const profiles = useApp((s) => s.profiles);
  const active = useApp((s) => s.activeProfile);
  const collections = useApp((s) => s.collections);
  const switchProfile = useApp((s) => s.switchProfile);
  const setProfiles = useApp((s) => s.setProfiles);
  const removeCollection = useApp((s) => s.removeCollection);
  const toast = useApp((s) => s.toast);
  const [pending, setPending] = useState<Pending>(null);
  const [newName, setNewName] = useState("");
  const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null);

  const apply = async (call: Promise<{ profiles: Profile[]; active: string }>) => {
    try {
      const p = await call;
      setProfiles(p.profiles, p.active);
    } catch (e) {
      toast(String(e), "error");
    }
  };

  const add = async () => {
    if (!newName.trim()) return;
    await apply(api.addProfile(newName));
    setNewName("");
  };

  const rename = async () => {
    if (!renaming || !renaming.name.trim()) return;
    await apply(api.renameProfile(renaming.id, renaming.name));
    setRenaming(null);
  };

  return (
    <>
      <Section title="Profili">
        {profiles.map((p) =>
          renaming?.id === p.id ? (
            <div key={p.id} className="setting-row is-static">
              <div className="setting-inline">
                <input
                  className="text-input"
                  data-nav
                  data-nav-id="rename-profile"
                  aria-label="Nuovo nome del profilo"
                  value={renaming.name}
                  onChange={(e) => setRenaming({ ...renaming, name: e.target.value })}
                  onKeyDown={(e) => e.key === "Enter" && void rename()}
                />
                <button type="button" className="btn" data-nav onClick={() => void rename()}>
                  Salva
                </button>
              </div>
            </div>
          ) : (
            <Row
              key={p.id}
              label={p.name}
              hint={p.id === active ? "Profilo in uso" : "Premi per usarlo, rinominarlo o eliminarlo"}
              onClick={() => setPending({ kind: "profile", profile: p })}
              control={<Avatar profile={p} />}
            />
          ),
        )}
        <div className="setting-row is-static">
          <div className="setting-text">
            <span className="setting-label">Nuovo profilo</span>
            <span className="setting-hint">Ognuno ha i suoi preferiti, raccolte e tempo di gioco</span>
          </div>
          <div className="setting-inline">
            <input
              className="text-input"
              data-nav
              aria-label="Nome del nuovo profilo"
              placeholder="Nome"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && void add()}
            />
            <button type="button" className="btn" data-nav onClick={() => void add()}>
              Aggiungi
            </button>
          </div>
        </div>
      </Section>

      {collections.length > 0 && (
        <Section title="Raccolte">
          {collections.map((c) => (
            <Row
              key={c.id}
              label={c.name}
              hint={`${c.gameIds.length} ${c.gameIds.length === 1 ? "gioco" : "giochi"} · aggiungi giochi dal loro dettaglio`}
              onClick={() => setPending({ kind: "deleteCollection", collection: c })}
              control={<span className="setting-action">Elimina</span>}
            />
          ))}
        </Section>
      )}

      <AnimatePresence>
        {pending?.kind === "profile" && (
          <ActionSheet
            key="profile"
            title={pending.profile.name}
            onClose={() => setPending(null)}
            actions={[
              ...(pending.profile.id !== active
                ? [{ label: "Usa questo profilo", onSelect: () => void switchProfile(pending.profile.id) }]
                : []),
              {
                label: "Rinomina",
                onSelect: () => {
                  setRenaming({ id: pending.profile.id, name: pending.profile.name });
                  requestAnimationFrame(() => document.querySelector<HTMLElement>("[data-nav-id='rename-profile']")?.focus());
                },
              },
              ...(profiles.length > 1
                ? [{ label: "Elimina", danger: true, onSelect: () => setPending({ kind: "deleteProfile", profile: pending.profile }) }]
                : []),
            ]}
          />
        )}
        {pending?.kind === "deleteProfile" && (
          <ConfirmDialog
            key="delete-profile"
            title={`Eliminare ${pending.profile.name}?`}
            text="Si perdono i suoi preferiti, raccolte e tempo di gioco. I giochi installati non vengono toccati."
            confirmLabel="Elimina"
            onCancel={() => setPending(null)}
            onConfirm={() => {
              const id = pending.profile.id;
              setPending(null);
              void apply(api.removeProfile(id)).then(() => requestAnimationFrame(ensureFocus));
            }}
          />
        )}
        {pending?.kind === "deleteCollection" && (
          <ConfirmDialog
            key="delete-collection"
            title={`Eliminare la raccolta ${pending.collection.name}?`}
            text="I giochi restano nella libreria, sparisce solo il gruppo."
            confirmLabel="Elimina"
            onCancel={() => setPending(null)}
            onConfirm={() => {
              const id = pending.collection.id;
              setPending(null);
              void removeCollection(id).then(() => requestAnimationFrame(ensureFocus));
            }}
          />
        )}
      </AnimatePresence>
    </>
  );
}
