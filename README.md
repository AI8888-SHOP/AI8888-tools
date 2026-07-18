# AI8888 Tools

AI8888 Tools is a Tauri desktop client for managing AI8888 and official OpenAI configuration across local coding tools. It synchronizes account, subscription, group, and API key information; manages routing, MCP, prompts, skills, projects, usage, backups, and local sessions; and ships cross-platform updates through GitHub Releases.

Current version: v0.1.0

## Features

- Account login and API key management for AI8888.
- Official Codex ChatGPT browser/device login, status, logout, and transactional OpenAI/AI8888 switching.
- Multi-tool configuration writing for Codex, Claude Code, OpenCode, OpenClaw, and Hermes.
- Endpoint probing across AI8888 domains for regions where some domains may be blocked.
- Local routing V2 with streaming passthrough, Anthropic/OpenAI tool and image conversion, endpoint priority, retries, circuit breaking, and health checks.
- Local usage and cost dashboard grouped by model, endpoint, and day, with editable model pricing. Request bodies and credentials are never logged.
- Unified MCP management for Codex, Claude, Gemini, OpenCode, OpenClaw, and Hermes, including live import and per-app synchronization.
- Managed Markdown prompts with non-destructive blocks for AGENTS.md, CLAUDE.md, and GEMINI.md style memory files.
- Skills installation from local directories, ZIP files, and GitHub repositories, with per-app copy synchronization and uninstall backups.
- Named projects that snapshot a Profile plus the active MCP, prompt, and skill set, with one-click switching from the app or system tray.
- Cross-tool session manager for Codex, Claude, Gemini, OpenCode, OpenClaw, and Hermes, including full-text search and resume commands.
- Dynamic system tray with official OpenAI status, AI8888 balance, Profile switching, project switching, and session access.
- Sanitized configuration export, AES-256-GCM encrypted secret backup/import, and a multi-tool diagnostic/repair center.
- GitHub Releases based update check from the main window footer.
- Mainland China download acceleration for GitHub release assets when the exit IP is CN.
- Streamed update download/install with progress, cancellation, accelerated-download fallback, digest verification, and failed-download cleanup.
- Transactional multi-tool configuration writes with automatic failure rollback and up to 20 versioned snapshots.
- Reusable configuration Profiles for endpoint, Key, model, target tool, and local-routing rules, with one-click transactional apply.
- Account usage and subscription expiry alerts with dismiss actions (auto-refresh every 15 minutes while logged in, plus on window focus).
- Codex session visibility repair and direct resume launch remain available inside the unified session manager.
- First-run setup wizard for endpoint probe, tool/key selection, and config write.

## Packaging policy

Local machines only write and push code.

GitHub Actions verifies every `master` push, builds Windows x64/ARM64, Linux x64/ARM64, and macOS universal installers, and publishes GitHub Releases only for version tags.

### Release flow

1. Commit and push code to `master`. GitHub Actions runs tests, checks, and cross-platform builds without publishing a release.
2. Create and push a version tag, for example:

```bash
git tag v0.1.0
git push origin v0.1.0
```

3. GitHub Actions workflow `Build and release desktop packages` will:
- build Windows x64/ARM64, Linux x64/ARM64, and macOS universal packages
- package portable ZIPs for both Windows architectures
- upload artifacts
- generate a downloadable Homebrew Cask from the universal DMG SHA-256
- verify per-architecture MSI, NSIS, portable ZIP, AppImage, deb, rpm, universal DMG, Homebrew Cask, and GitHub SHA-256 digests
- create a draft Release and publish it only after all asset checks pass

Manual workflow runs only build artifacts and never publish. Release tags must exactly match the version embedded in npm, Cargo, Tauri, Rust, and the renderer.

macOS signing and notarization are enabled automatically when the repository provides all `APPLE_*` signing secrets; otherwise the workflow produces an unsigned DMG.

Homebrew users can install the universal macOS build from the generated Cask asset:

```bash
curl -LO https://github.com/AI8888-SHOP/AI8888-tools/releases/latest/download/ai8888-switch.rb
brew install --cask ./ai8888-switch.rb
```

## Open Source Acknowledgements

This project thanks and references ideas from these open source projects:

- cockpit-tools: https://github.com/jlcodes99/cockpit-tools
- cc-switch: https://github.com/farion1231/cc-switch
- sub2api: https://github.com/Wei-Shaw/sub2api

## Development

Local machines edit, commit, and push source code. Compilation, tests, packaging, and release publication run in GitHub Actions.
