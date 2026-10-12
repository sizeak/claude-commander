import { type Api, act } from "./api.ts";
import { byId, closeModal, openModal } from "./dom.ts";
import type { EditSession, ProgramInfo, SessionInfo } from "./generated/index.ts";
import { state } from "./state.ts";

let api: Api;
let refresh: () => Promise<void>;
let onRestart: (id: string) => void;
let session: SessionInfo | null = null;
let pending: EditSession | null = null;
let busy = false;
let originalParent = "";
const modal = () => byId("edit-modal");
const field = (id: string) => byId<HTMLInputElement>(`edit-${id}`);
const select = (id: string) => byId<HTMLSelectElement>(`edit-${id}`);

/** Show aliases and retain both live and pending commands. */
export function programChoices(
  programs: ProgramInfo[],
  current: string,
  live = current,
): ProgramInfo[] {
  const choices = new Map<string, ProgramInfo>();
  for (const program of programs) {
    if (!choices.has(program.command)) choices.set(program.command, program);
  }
  for (const command of [current, live]) {
    if (!choices.has(command)) choices.set(command, { label: command, command });
  }
  return [...choices.values()];
}

/** Build one request from the form. Only an explicitly changed base is retargeted. */
export function sessionEdit(
  info: SessionInfo,
  title: string,
  program: string,
  section: string,
  keepAlive: boolean,
  parent: string,
  originalBase = info.stack_parent_session_id ?? "",
): EditSession {
  return {
    title: title.trim(),
    program: program.trim(),
    section: section || null,
    keep_alive: keepAlive,
    restart: false,
    base: parent === originalBase ? null : { parent_session_id: parent || null },
  };
}

export function initSessionEditor(
  a: Api,
  reload: () => Promise<void>,
  restarted: (id: string) => void,
): void {
  api = a;
  refresh = reload;
  onRestart = restarted;
  byId<HTMLFormElement>("edit-form").addEventListener("submit", (e) => {
    e.preventDefault();
    if (!session || busy) return;
    pending = sessionEdit(
      session,
      field("title").value,
      select("program").value,
      select("section").value,
      field("keep-alive").checked,
      select("base").value,
      originalParent,
    );
    if (!pending.title || !pending.program) return;
    if (pending.program !== (session.pending_program ?? session.program)) {
      byId("edit-fields").classList.add("hidden");
      byId("edit-warning").classList.remove("hidden");
      byId("edit-heading").textContent = "Restart session?";
      byId("edit-later").focus();
    } else void save(false);
  });
  byId("edit-now").addEventListener("click", () => void save(true));
  byId("edit-later").addEventListener("click", () => void save(false));
  byId("edit-back").addEventListener("click", showFields);
}
function showFields(): void {
  byId("edit-fields").classList.remove("hidden");
  byId("edit-warning").classList.add("hidden");
  byId("edit-heading").textContent = "Edit session";
}
async function save(restart: boolean): Promise<void> {
  if (!session || !pending || busy) return;
  const id = session.id;
  busy = true;
  const buttons = modal().querySelectorAll<HTMLButtonElement>("button");
  for (const b of buttons) b.disabled = true;
  try {
    const result = await act(api.editSession(id, { ...pending, restart }));
    if (!result) return;
    if (result.value?.pr.kind === "failed")
      alert(`Session saved, but PR target update failed: ${result.value.pr.message}`);
    closeModal(modal());
    if (restart) onRestart(id);
    await refresh();
  } finally {
    busy = false;
    for (const b of buttons) b.disabled = false;
  }
}
export async function openSessionEditor(info: SessionInfo): Promise<void> {
  if (busy) return;
  session = info;
  pending = null;
  showFields();
  field("title").value = info.title;
  const program = select("program");
  const currentProgram = info.pending_program ?? info.program;
  program.replaceChildren(
    ...programChoices([], currentProgram, info.program).map((p) => new Option(p.label, p.command)),
  );
  program.value = currentProgram;
  field("keep-alive").checked = info.keep_alive;
  const sections = select("section");
  sections.replaceChildren(new Option("Automatic", ""));
  if (info.section_override) sections.add(new Option(info.section_override, info.section_override));
  sections.value = info.section_override ?? "";
  const base = select("base");
  base.replaceChildren(new Option("Project base", ""));
  for (const s of state.sessions) {
    if (s.project_id === info.project_id && s.id !== info.id)
      base.add(new Option(`${s.title} (${s.branch})`, s.id));
  }
  originalParent = info.pr_base_branch
    ? (state.sessions.find(
        (s) =>
          s.project_id === info.project_id && s.id !== info.id && s.branch === info.pr_base_branch,
      )?.id ?? "__current__")
    : (info.stack_parent_session_id ?? "");
  if (
    originalParent === "__current__" ||
    (originalParent && ![...base.options].some((o) => o.value === originalParent))
  ) {
    base.add(
      new Option(`Current base (${info.pr_base_branch ?? "session unavailable"})`, originalParent),
    );
  }
  base.value = originalParent;
  byId("edit-options-error").textContent = "";
  openModal(modal());
  field("title").focus();
  try {
    const options = await api.createOptions();
    if (session !== info) return;
    const current = sections.value;
    for (const s of options.sections) {
      if (![...sections.options].some((o) => o.value === s)) sections.add(new Option(s, s));
    }
    sections.value = current;
    const selectedProgram = program.value;
    program.replaceChildren(
      ...programChoices(options.programs, currentProgram, info.program).map(
        (p) => new Option(p.label, p.command),
      ),
    );
    program.value = selectedProgram;
  } catch {
    if (session === info)
      byId("edit-options-error").textContent =
        "Could not load program and section choices. Existing settings can still be saved.";
  }
}
