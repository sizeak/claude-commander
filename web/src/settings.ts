// The settings modal: reads `GET /config`, patches back only the fields it
// shows (the rest of `ConfigPatch` is left absent, i.e. unchanged).

import { type Api, act, Unauthorized } from "./api.ts";
import { byId, closeModal, els, openModal } from "./dom.ts";
import type { ConfigPatch } from "./generated/index.ts";

const field = {
  get branchPrefix() {
    return byId<HTMLInputElement>("set-branch-prefix");
  },
  get fetchBeforeCreate() {
    return byId<HTMLInputElement>("set-fetch-before-create");
  },
  get resumeSession() {
    return byId<HTMLInputElement>("set-resume-session");
  },
  get projectPullEnabled() {
    return byId<HTMLInputElement>("set-project-pull-enabled");
  },
};

export function initSettings(api: Api): void {
  els.settingsBtn.addEventListener("click", async () => {
    try {
      const c = await api.config();
      field.branchPrefix.value = c.branch_prefix ?? "";
      field.fetchBeforeCreate.checked = !!c.fetch_before_create;
      field.resumeSession.checked = !!c.resume_session;
      field.projectPullEnabled.checked = !!c.project_pull_enabled;
      openModal(els.settingsModal);
    } catch (e) {
      if (!(e instanceof Unauthorized)) alert(`Failed to load settings: ${(e as Error).message}`);
    }
  });

  els.settingsForm.addEventListener("submit", async (e) => {
    e.preventDefault();
    const patch: ConfigPatch = {
      branch_prefix: field.branchPrefix.value,
      fetch_before_create: field.fetchBeforeCreate.checked,
      resume_session: field.resumeSession.checked,
      project_pull_enabled: field.projectPullEnabled.checked,
    };
    if (await act(api.patchConfig(patch))) closeModal(els.settingsModal);
  });
}
