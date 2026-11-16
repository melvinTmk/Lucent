// Power actions shared by the Control Center and the search page. The names are the wire
// names of Rust's `power_action` command. Restart, shut down and sign out are destructive:
// callers must confirm first. Never bound to a hotkey.
import { invoke } from "./ipc";
import { t } from "./i18n";

export type PowerId = "lock" | "sleep" | "signout" | "restart" | "shutdown";

export const POWER_IDS: readonly PowerId[] = ["lock", "sleep", "restart", "shutdown", "signout"];

const DESTRUCTIVE: ReadonlySet<string> = new Set(["signout", "restart", "shutdown"]);

export function isDestructive(id: string): boolean {
  return DESTRUCTIVE.has(id);
}

export function isPowerId(id: string): id is PowerId {
  return (POWER_IDS as readonly string[]).includes(id);
}

export function powerLabel(id: PowerId): string {
  return t.power[id];
}

export function confirmText(id: PowerId): string | null {
  switch (id) {
    case "restart":
      return t.power.confirmRestart;
    case "shutdown":
      return t.power.confirmShutdown;
    case "signout":
      return t.power.confirmSignout;
    default:
      return null;
  }
}

export type PowerExecutor = (id: PowerId) => Promise<unknown>;

let executor: PowerExecutor = (id) => invoke("power_action", { action: id });

/** Tests swap the executor: power actions are never executed for real there. */
export function setPowerExecutor(fn: PowerExecutor | null): void {
  executor = fn ?? ((id) => invoke("power_action", { action: id }));
}

export function runPower(id: PowerId): Promise<unknown> {
  return executor(id);
}

/** Two-step confirmation with a timeout (search: Enter, then Enter again within `ms`). */
export interface ConfirmGate {
  /** True when this call confirms (armed on the same id and not expired); else arms and returns false. */
  press(id: string): boolean;
  armed(): string | null;
  reset(): void;
}

export function createConfirmGate(ms: number, now: () => number = () => performance.now()): ConfirmGate {
  let id: string | null = null;
  let until = 0;
  return {
    press(next) {
      if (id === next && now() <= until) {
        id = null;
        return true;
      }
      id = next;
      until = now() + ms;
      return false;
    },
    armed() {
      if (id !== null && now() > until) id = null;
      return id;
    },
    reset() {
      id = null;
    },
  };
}
