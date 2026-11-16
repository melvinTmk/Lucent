# Contributing to Lucent

Thanks for helping! Issues and pull requests can be written in English or French.

## Reporting a bug

Open an issue with:
- your Windows version, number of monitors and their scaling;
- what you did, what you expected, what happened;
- the end of `%LOCALAPPDATA%\Lucent\logs\lucent.log` (it never contains your API
  key; check it for anything you would rather not share).

## Development

Everything builds and runs on Windows (see "Build from source" in the README). From WSL,
call the Windows toolchain (`cmd.exe /c "cd /d C:\path\to\lucent && npm test"`).

Before opening a pull request:

```powershell
npx tsc --noEmit
npm test
cd src-tauri; cargo test
```

Guidelines:
- **No personal data in code**: no hardcoded paths, accounts, monitors or game ids.
  Anything machine-specific is a setting or is detected.
- **Never block the main thread.** It owns every window: blocking calls (shell, disk,
  network, DDC/CI, COM) run on a worker or in an `async` command with `spawn_blocking`.
- **The Windows taskbar must always come back** (exit, crash, kill, uninstall). Changes
  around `src-tauri/src/taskbar/` need a manual test with the release exe.
- Pure logic goes in testable functions, Win32 calls stay thin in `#[cfg(windows)] mod win`.
- User-facing strings are French, in `src/shared/i18n.ts` (frontend) or next to the Rust
  code that shows them.

## Translations

The interface is French only for now. A translation starts with `src/shared/i18n.ts`
(keep the same keys) plus the few Rust-side strings (search commands, power menu, cards).
Open an issue first so we can agree on how the language is picked.

## License

By contributing, you agree that your contributions are licensed under the GPL-3.0-or-later,
like the rest of the project.
