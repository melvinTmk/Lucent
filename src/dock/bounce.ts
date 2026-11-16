export interface BounceOptions {
  timeoutMs?: number;
  onChange?: () => void;
}

export interface BounceManager {
  trigger(id: string): void;
  clear(id: string): void;
  isBouncing(id: string): boolean;
  update(items: Array<{ id: string; windows: unknown[] }>): void;
  bouncingIds(): string[];
  destroy(): void;
}

export function createBounceManager(options: BounceOptions = {}): BounceManager {
  const timeoutMs = options.timeoutMs ?? 10_000;
  const onChange = options.onChange;

  const bouncing = new Set<string>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();

  return {
    trigger(id: string): void {
      if (bouncing.has(id)) return;
      bouncing.add(id);

      const timer = setTimeout(() => {
        timers.delete(id);
        bouncing.delete(id);
        onChange?.();
      }, timeoutMs);

      timers.set(id, timer);
      onChange?.();
    },

    clear(id: string): void {
      const timer = timers.get(id);
      if (timer) {
        clearTimeout(timer);
        timers.delete(id);
      }
      if (bouncing.delete(id)) {
        onChange?.();
      }
    },

    isBouncing(id: string): boolean {
      return bouncing.has(id);
    },

    update(items: Array<{ id: string; windows: unknown[] }>): void {
      for (const item of items) {
        if (bouncing.has(item.id) && item.windows.length > 0) {
          const timer = timers.get(item.id);
          if (timer) {
            clearTimeout(timer);
            timers.delete(item.id);
          }
          bouncing.delete(item.id);
          onChange?.();
        }
      }
    },

    bouncingIds(): string[] {
      return Array.from(bouncing);
    },

    destroy(): void {
      for (const timer of timers.values()) {
        clearTimeout(timer);
      }
      timers.clear();
      bouncing.clear();
    },
  };
}
