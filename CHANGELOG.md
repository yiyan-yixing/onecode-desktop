# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] — 2025-06-28

### Added

- Multi-kernel AI terminal manager — manage Claude Code, OpenCode, Codex, Crush, Aider, Goose, Hermes terminals in one window
- @Mention popup with cursor-position-based placement (auto-flip on overflow)
- Project card right-click context menu (open terminal, show in Finder, delete)
- Custom confirmation dialog (replaces native `confirm()`)
- IPC disconnection detection banner (3+ consecutive failures)
- Palette settings save feedback (✓/✗ visual indicators)
- Project list loading state
- Ambient controller — statusbar fades after 8s idle
- Sensitive directory protection in file explorer (~/.ssh, ~/.gnupg, etc.)
- Session DB transaction wrapping (crash-safe persistence)
- Generation counter for PTY restart — eliminates race condition
- `recover_lock!` macro — poisoned mutex recovery instead of cascade crash

### Fixed

- @Mention popup rendered at 0,0 (no positioning)
- Project card right-click menu unreachable (`_showProjectCtxMenu` was dead code)
- CSP disabled in release builds
- DevTools open in production builds
- Session DB non-transactional write — data loss on crash
- restart() 80ms sleep race condition — two wait threads could coexist
- 33 `.expect()` on Mutex — thread panic would cascade-crash the app
- `kill_all_blocking` could skip killing terminals when locks contended
- `run_with_timeout` didn't enforce timeout — wizard could hang
- File explorer could read sensitive directories
- Palette orb icon color values broken (raw CSS var without `var()`)
- WebGL addon loaded but never used (dead weight)
- File explorer auto-refreshed even when sidebar collapsed
- Error tab ID collision (Date.now() same millisecond)
- 11 clippy warnings

### Changed

- Product positioning from "Claude Code shell" to "multi-kernel AI terminal manager"
- tauri.conf.json: CSP enabled, devtools disabled in production, publisher/copyright added
- CI now runs `cargo test` and `npm test`
- Release workflow has code signing + notarization placeholder steps

### Removed

- `console-capture.js` (dev-only utility, never loaded in HTML)
