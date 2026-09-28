// What the header's connection indicator shows. Pure; dom.ts renders it.
//
// Three layers, highest first:
//   flash  — a transient note ("copied"), until its timer ends it;
//   sticky — a terminal state of the attach ("session ended", a rejected
//            token), until the user acts (selects, toggles, reconnects) —
//            except that an error base ("disconnected") shows through it;
//   base   — the poll's own view ("connected" / "disconnected").
// The poll writes only the base layer, every 1.5s. With a single slot, as
// before, each poll overwrote whatever the terminal had said a moment earlier.

export type ConnClass = "ok" | "error" | "unknown";

export interface ConnView {
  cls: ConnClass;
  text: string;
}

/** Identifies one flash, so ending an older one leaves a newer showing. */
export type FlashId = number;

export class ConnStatus {
  private base: ConnView = { cls: "unknown", text: "connecting…" };
  private sticky: ConnView | null = null;
  private flashView: { id: FlashId; view: ConnView } | null = null;
  private nextFlash = 1;

  setBase(cls: ConnClass, text: string): void {
    this.base = { cls, text };
  }

  setSticky(cls: ConnClass, text: string): void {
    this.sticky = { cls, text };
  }

  clearSticky(): void {
    this.sticky = null;
  }

  flash(cls: ConnClass, text: string): FlashId {
    const id = this.nextFlash++;
    this.flashView = { id, view: { cls, text } };
    return id;
  }

  endFlash(id: FlashId): void {
    if (this.flashView?.id === id) this.flashView = null;
  }

  view(): ConnView {
    if (this.flashView) return this.flashView.view;
    // Losing the server outranks a sticky state: that is about one pane, this
    // about everything, and hiding it would claim a server we can't reach.
    if (this.base.cls === "error") return this.base;
    return this.sticky ?? this.base;
  }
}
