import type { ReactNode } from "react";

export function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="settings-section">
      <h2 className="row-title">{title}</h2>
      <div className="settings-list">{children}</div>
    </section>
  );
}

export function Row(props: { label: string; hint?: string; control?: ReactNode; autofocus?: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      className="setting-row"
      data-nav
      data-nav-autofocus={props.autofocus || undefined}
      onClick={props.onClick}
    >
      <span className="setting-text">
        <span className="setting-label">{props.label}</span>
        {props.hint && <span className="setting-hint">{props.hint}</span>}
      </span>
      {props.control}
    </button>
  );
}
