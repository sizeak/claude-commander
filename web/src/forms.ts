// The new-session and add/scan-project modals.

import { type Api, act, Unauthorized } from "./api.ts";
import { closeModal, els, h, openModal } from "./dom.ts";
import type { CreateOptions, CreateSessionOpts, ProjectInfo } from "./generated/index.ts";
import { currentSession, state } from "./state.ts";

export interface FormHooks {
  refresh(): Promise<void>;
  select(id: string): void;
}

let api: Api;
let hooks: FormHooks;

export function initForms(a: Api, h_: FormHooks): void {
  api = a;
  hooks = h_;

  els.newBtn.addEventListener("click", () => {
    if (state.projects.length === 0) {
      alert("Add a project first.");
      openModal(els.projectModal);
      return;
    }
    const sel = currentSession();
    const target =
      (sel && state.projects.find((p) => p.id === sel.project_id)) || state.projects[0];
    if (target) openNewSession(target);
  });
  els.newForm.addEventListener("submit", submitNewSession);

  els.addProjectBtn.addEventListener("click", () => {
    els.projectPath.value = "";
    openModal(els.projectModal);
    els.projectPath.focus();
  });
  els.addPathBtn.addEventListener("click", addProject);
  els.scanDirBtn.addEventListener("click", scanProjects);
}

// ---- new session -------------------------------------------------------------

function fillSelect(sel: HTMLSelectElement, values: readonly string[], placeholder: string): void {
  sel.replaceChildren();
  const empty = h("option", { text: placeholder });
  empty.value = "";
  sel.append(empty);
  for (const v of values) {
    const o = h("option", { text: v });
    o.value = v;
    sel.append(o);
  }
}

async function ensureCreateOptions(): Promise<CreateOptions> {
  if (!state.createOptions) state.createOptions = await api.createOptions();
  return state.createOptions;
}

export async function openNewSession(project: ProjectInfo): Promise<void> {
  try {
    const opts = await ensureCreateOptions();
    els.newProject.value = project.repo_path;
    els.newProjectName.value = project.name;
    fillSelect(els.newSection, opts.sections ?? [], "(auto)");
    els.newProgram.placeholder = opts.default_program || "(default)";
    openModal(els.newModal);
    els.newTitle.focus();
  } catch (e) {
    if (!(e instanceof Unauthorized)) alert(`Failed to load form options: ${(e as Error).message}`);
  }
}

/** Empty (after trimming) → null, the wire's "use the server default". */
function orNull(v: string): string | null {
  return v.trim() || null;
}

async function submitNewSession(e: SubmitEvent): Promise<void> {
  e.preventDefault();
  // Effort, permission mode and model are left to the server's defaults: the
  // server does not enumerate their valid values, and a hand-kept list here
  // was already wrong (it lacked `auto`).
  const body: CreateSessionOpts = {
    project_path: els.newProject.value,
    title: els.newTitle.value.trim(),
    program: orNull(els.newProgram.value),
    initial_prompt: orNull(els.newPrompt.value),
    effort: null,
    mode: null,
    model: null,
    base_branch: orNull(els.newBase.value),
    section: els.newSection.value || null,
    stack_parent: null,
  };
  const res = await act(api.createSession(body));
  if (!res) return;
  closeModal(els.newModal);
  els.newForm.reset();
  await hooks.refresh();
  if (res.value?.id) hooks.select(res.value.id);
}

// ---- projects ----------------------------------------------------------------

async function addProject(): Promise<void> {
  const path = els.projectPath.value.trim();
  if (!path) return;
  if (await act(api.addProject({ path }))) {
    closeModal(els.projectModal);
    hooks.refresh();
  }
}

async function scanProjects(): Promise<void> {
  const path = els.projectPath.value.trim();
  if (!path) return;
  const res = await act(api.scanProjects({ path }));
  if (!res) return;
  closeModal(els.projectModal);
  alert(`Scan complete: ${res.value.added} added, ${res.value.skipped} skipped.`);
  hooks.refresh();
}

export async function removeProject(p: ProjectInfo): Promise<void> {
  if (!confirm(`Remove project "${p.name}"? This deletes all its sessions and worktrees.`)) return;
  if (await act(api.removeProject(p.id))) hooks.refresh();
}
